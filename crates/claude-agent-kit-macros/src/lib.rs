//! Procedural macros for `claude-agent-kit`.
//!
//! The macros are re-exported by `claude-agent-kit` behind its `macros`
//! feature and are documented there. Depend on that crate, not on this one.

#![deny(missing_docs)]
#![deny(unsafe_code)]

use proc_macro::TokenStream;
use proc_macro2::TokenStream as TokenStream2;
use quote::{ToTokens, quote};
use syn::parse::Parser;
use syn::{
    Attribute, Error, Expr, FnArg, ItemFn, Lit, LitBool, LitInt, LitStr, Meta, Result, Type,
};

/// Turns an async function into a function returning the tool it defines.
///
/// See the documentation of `claude_agent_kit::tool`.
#[proc_macro_attribute]
pub fn tool(arguments: TokenStream, item: TokenStream) -> TokenStream {
    expand(arguments.into(), item.into())
        .unwrap_or_else(Error::into_compile_error)
        .into()
}

#[derive(Default)]
struct Settings {
    name: Option<LitStr>,
    description: Option<LitStr>,
    title: Option<LitStr>,
    read_only_hint: Option<LitBool>,
    destructive_hint: Option<LitBool>,
    idempotent_hint: Option<LitBool>,
    open_world_hint: Option<LitBool>,
    max_result_size_chars: Option<LitInt>,
}

impl Settings {
    fn parse(arguments: TokenStream2) -> Result<Self> {
        let mut settings = Self::default();
        let parser = syn::meta::parser(|meta| {
            let key = meta
                .path
                .get_ident()
                .map(ToString::to_string)
                .unwrap_or_default();
            match key.as_str() {
                "name" => settings.name = Some(meta.value()?.parse()?),
                "description" => settings.description = Some(meta.value()?.parse()?),
                "title" => settings.title = Some(meta.value()?.parse()?),
                "read_only_hint" => settings.read_only_hint = Some(meta.value()?.parse()?),
                "destructive_hint" => settings.destructive_hint = Some(meta.value()?.parse()?),
                "idempotent_hint" => settings.idempotent_hint = Some(meta.value()?.parse()?),
                "open_world_hint" => settings.open_world_hint = Some(meta.value()?.parse()?),
                "max_result_size_chars" => {
                    settings.max_result_size_chars = Some(meta.value()?.parse()?);
                }
                _ => {
                    return Err(meta.error(
                        "unknown #[tool] argument: expected `name`, `description`, `title`, \
                         `read_only_hint`, `destructive_hint`, `idempotent_hint`, \
                         `open_world_hint`, or `max_result_size_chars`",
                    ));
                }
            }
            Ok(())
        });
        parser.parse2(arguments)?;
        Ok(settings)
    }

    fn annotations(&self) -> Option<TokenStream2> {
        let mut setters = Vec::new();
        if let Some(title) = &self.title {
            setters.push(quote!(.title(#title)));
        }
        for (hint, setter) in [
            (&self.read_only_hint, quote!(read_only_hint)),
            (&self.destructive_hint, quote!(destructive_hint)),
            (&self.idempotent_hint, quote!(idempotent_hint)),
            (&self.open_world_hint, quote!(open_world_hint)),
        ] {
            if let Some(hint) = hint {
                setters.push(quote!(.#setter(#hint)));
            }
        }
        if let Some(size) = &self.max_result_size_chars {
            setters.push(quote!(.max_result_size_chars(#size)));
        }
        if setters.is_empty() {
            return None;
        }
        Some(quote! {
            .annotations(::claude_agent_kit::ToolAnnotations::new() #(#setters)*)
        })
    }
}

fn doc_comment(attributes: &[Attribute]) -> Option<String> {
    let lines: Vec<String> = attributes
        .iter()
        .filter(|attribute| attribute.path().is_ident("doc"))
        .filter_map(|attribute| match &attribute.meta {
            Meta::NameValue(pair) => match &pair.value {
                Expr::Lit(literal) => match &literal.lit {
                    Lit::Str(text) => Some(text.value().trim().to_owned()),
                    _ => None,
                },
                _ => None,
            },
            _ => None,
        })
        .collect();
    let text = lines.join("\n").trim().to_owned();
    (!text.is_empty()).then_some(text)
}

fn argument_type(function: &ItemFn) -> Result<&Type> {
    let name = &function.sig.ident;
    let inputs = &function.sig.inputs;
    let count = inputs.len();
    let wrong_count = || {
        let found = match count {
            0 => "no argument".to_owned(),
            count => format!("{count} arguments"),
        };
        Error::new_spanned(
            &function.sig,
            format!(
                "#[tool] expects exactly one argument struct, but `{name}` takes {found}: \
                 declare it as `async fn {name}(args: Args) -> Result<ToolResult, E>`"
            ),
        )
    };
    if count != 1 {
        return Err(wrong_count());
    }
    match inputs.first() {
        Some(FnArg::Typed(argument)) => Ok(&argument.ty),
        Some(FnArg::Receiver(receiver)) => Err(Error::new_spanned(
            receiver,
            format!(
                "#[tool] expects exactly one argument struct, but `{name}` takes `self`: a \
                 tool is a free function"
            ),
        )),
        None => Err(wrong_count()),
    }
}

fn expand(arguments: TokenStream2, item: TokenStream2) -> Result<TokenStream2> {
    let settings = Settings::parse(arguments)?;
    let function: ItemFn = syn::parse2(item)?;
    let ident = &function.sig.ident;

    if function.sig.asyncness.is_none() {
        return Err(Error::new_spanned(
            function.sig.fn_token,
            format!(
                "`{ident}` must be async: #[tool] turns an \
                 `async fn {ident}(args: Args) -> Result<ToolResult, E>` into a tool"
            ),
        ));
    }
    if !function.sig.generics.params.is_empty() {
        return Err(Error::new_spanned(
            &function.sig.generics,
            format!("`{ident}` must not be generic: a tool has one concrete argument type"),
        ));
    }
    let argument = argument_type(&function)?;

    let name = settings
        .name
        .clone()
        .unwrap_or_else(|| LitStr::new(&ident.to_string(), ident.span()));
    let description = match (&settings.description, doc_comment(&function.attrs)) {
        (Some(description), _) => description.to_token_stream(),
        (None, Some(documentation)) => LitStr::new(&documentation, ident.span()).to_token_stream(),
        (None, None) => {
            return Err(Error::new_spanned(
                ident,
                format!(
                    "`{ident}` needs a description: add a doc comment or \
                     `description = \"...\"` to #[tool]"
                ),
            ));
        }
    };
    let annotations = settings.annotations();

    let attributes = &function.attrs;
    let visibility = &function.vis;
    let signature = &function.sig;
    let block = &function.block;
    Ok(quote! {
        #(#attributes)*
        #visibility fn #ident() -> ::claude_agent_kit::SdkMcpTool {
            #signature #block

            ::claude_agent_kit::SdkMcpTool::typed::<#argument>(
                #name,
                #description,
                |__claude_agent_kit_arguments| async move {
                    #ident(__claude_agent_kit_arguments)
                        .await
                        .map_err(::claude_agent_kit::BoxError::from)
                },
            )
            #annotations
        }
    })
}
