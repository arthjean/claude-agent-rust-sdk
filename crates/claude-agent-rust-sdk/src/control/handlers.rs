//! Handlers of the control requests the CLI sends: tool permission, hook
//! callbacks, and messages for the in-process MCP servers.

use std::panic::AssertUnwindSafe;
use std::sync::Arc;

use futures_util::FutureExt;
use serde_json::{Value, json};
use tokio::sync::oneshot;

use super::Shared;
use crate::types::{
    HookContext, HookInput, PermissionResult, PermissionUpdate, ToolPermissionContext,
};

const MISSING_PERMISSION_CALLBACK: &str = "canUseTool callback is not provided";

/// Starts one handler task for a control request and tracks it by request
/// id, so a cancel request can abort it.
///
/// A cancelled handler writes no response: the CLI has already abandoned
/// the request. Cancellation stops the application callback only; once the
/// callback has answered, the response is written in full.
pub(super) fn spawn(shared: &Arc<Shared>, frame: Value) {
    let Some(request_id) = frame
        .get("request_id")
        .and_then(Value::as_str)
        .map(str::to_owned)
    else {
        tracing::warn!("Ignoring a control request without a request id");
        return;
    };
    let (cancel, cancelled) = oneshot::channel::<()>();
    {
        let mut state = shared.state();
        if state.closed {
            return;
        }
        state.inflight.insert(request_id.clone(), cancel);
    }
    let shared = Arc::clone(shared);
    tokio::spawn(async move {
        let request = frame.get("request").cloned().unwrap_or(Value::Null);
        let outcome = tokio::select! {
            biased;
            _ = cancelled => None,
            outcome = AssertUnwindSafe(answer(&shared, &request)).catch_unwind() => Some(outcome),
        };
        shared.state().inflight.remove(&request_id);
        let Some(outcome) = outcome else {
            return;
        };
        let response = match outcome {
            Ok(Ok(response)) => json!({
                "subtype": "success",
                "request_id": request_id,
                "response": response,
            }),
            Ok(Err(error)) => json!({
                "subtype": "error",
                "request_id": request_id,
                "error": error,
            }),
            Err(_) => json!({
                "subtype": "error",
                "request_id": request_id,
                "error": "The control request handler panicked",
            }),
        };
        let frame = json!({"type": "control_response", "response": response});
        if let Err(error) = shared.write_frame(&frame).await {
            tracing::debug!("Could not answer control request {request_id}: {error}");
        }
    });
}

/// Aborts the handler of a control request, if it is still running.
pub(super) fn cancel(shared: &Shared, frame: &Value) {
    let Some(request_id) = frame.get("request_id").and_then(Value::as_str) else {
        return;
    };
    if let Some(cancel) = shared.state().inflight.remove(request_id) {
        let _ = cancel.send(());
    }
}

async fn answer(shared: &Shared, request: &Value) -> Result<Value, String> {
    match request.get("subtype").and_then(Value::as_str) {
        Some("can_use_tool") => can_use_tool(shared, request).await,
        Some("hook_callback") => hook_callback(shared, request).await,
        Some("mcp_message") => mcp_message(shared, request).await,
        Some(subtype) => Err(format!("Unsupported control request subtype: {subtype}")),
        None => Err(format!(
            "Unsupported control request subtype: {}",
            request.get("subtype").unwrap_or(&Value::Null)
        )),
    }
}

fn required<'a>(request: &'a Value, subtype: &str, field: &str) -> Result<&'a Value, String> {
    request
        .get(field)
        .ok_or_else(|| format!("Missing required field in {subtype} request: '{field}'"))
}

async fn can_use_tool(shared: &Shared, request: &Value) -> Result<Value, String> {
    let callback = shared
        .config
        .can_use_tool
        .as_ref()
        .ok_or_else(|| MISSING_PERMISSION_CALLBACK.to_owned())?;
    let tool_name = required(request, "can_use_tool", "tool_name")?
        .as_str()
        .ok_or_else(|| "Invalid can_use_tool request: 'tool_name' must be a string".to_owned())?;
    let original_input = required(request, "can_use_tool", "input")?.clone();
    let suggestions = match request.get("permission_suggestions") {
        Some(Value::Array(suggestions)) => suggestions
            .iter()
            .map(PermissionUpdate::from_wire)
            .collect::<Result<Vec<_>, _>>()
            .map_err(|error| error.to_string())?,
        _ => Vec::new(),
    };
    let text = |field: &str| {
        request
            .get(field)
            .and_then(Value::as_str)
            .map(str::to_owned)
    };
    let context = ToolPermissionContext {
        suggestions,
        tool_use_id: text("tool_use_id"),
        agent_id: text("agent_id"),
        blocked_path: text("blocked_path"),
        decision_reason: text("decision_reason"),
        title: text("title"),
        display_name: text("display_name"),
        description: text("description"),
    };

    let decision = callback
        .call(tool_name.to_owned(), original_input.clone(), context)
        .await
        .map_err(|error| error.to_string())?;
    Ok(match decision {
        PermissionResult::Allow {
            updated_input,
            updated_permissions,
        } => {
            let mut response = json!({
                "behavior": "allow",
                "updatedInput": updated_input.unwrap_or(original_input),
            });
            if let Some(permissions) = updated_permissions
                .as_deref()
                .and_then(permission_updates_to_wire)
            {
                response["updatedPermissions"] = Value::Array(permissions);
            }
            response
        }
        PermissionResult::Deny { message, interrupt } => {
            let mut response = json!({"behavior": "deny", "message": message});
            if interrupt {
                response["interrupt"] = Value::Bool(true);
            }
            response
        }
    })
}

/// Converts the permission updates of an allow decision, all or none.
///
/// Returns `None`, and logs a warning, when any update cannot be converted:
/// the one that failed may be a restriction the others were meant to come
/// with.
fn permission_updates_to_wire(updates: &[PermissionUpdate]) -> Option<Vec<Value>> {
    let mut wire = Vec::with_capacity(updates.len());
    for (index, update) in updates.iter().enumerate() {
        match update.to_wire() {
            Ok(value) => wire.push(value),
            Err(error) => {
                tracing::warn!(
                    "The can_use_tool callback allowed the tool call, but none of its \
                     permission updates were applied, because updated_permissions[{index}] \
                     cannot be sent: {error}"
                );
                return None;
            }
        }
    }
    Some(wire)
}

async fn hook_callback(shared: &Shared, request: &Value) -> Result<Value, String> {
    let callback_id = required(request, "hook_callback", "callback_id")?
        .as_str()
        .unwrap_or_default();
    let callback = shared
        .config
        .hook_callbacks
        .get(callback_id)
        .ok_or_else(|| format!("No hook callback found for ID: {callback_id}"))?;
    let input = HookInput::from_wire(request.get("input").cloned().unwrap_or(Value::Null));
    let tool_use_id = request
        .get("tool_use_id")
        .and_then(Value::as_str)
        .map(str::to_owned);
    let output = callback
        .call(input, tool_use_id, HookContext::default())
        .await
        .map_err(|error| error.to_string())?;
    Ok(output.to_wire())
}

async fn mcp_message(shared: &Shared, request: &Value) -> Result<Value, String> {
    let server_name = request
        .get("server_name")
        .and_then(Value::as_str)
        .filter(|name| !name.is_empty());
    let message = request
        .get("message")
        .filter(|message| message.as_object().is_some_and(|fields| !fields.is_empty()));
    let (Some(server_name), Some(message)) = (server_name, message) else {
        return Err("Missing server_name or message for MCP request".to_owned());
    };
    let response = shared.config.mcp.handle(server_name, message).await;
    Ok(json!({"mcp_response": response}))
}
