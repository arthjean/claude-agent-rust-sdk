//! Procedural macros for `claude-agent-rust-sdk`.
//!
//! The macros are re-exported by `claude-agent-rust-sdk` behind its `macros`
//! feature and are documented there. Depend on that crate, not on this one.

#![deny(missing_docs)]
#![deny(unsafe_code)]

use proc_macro::TokenStream;
use proc_macro2::TokenStream as TokenStream2;
use quote::{ToTokens, quote};
use syn::parse::Parser;
use syn::{
    Attribute, Error, Expr, FnArg, Ident, ItemFn, Lit, LitBool, LitInt, LitStr, Meta, Pat, PatType,
    Path, Result, Type,
};

/// Turns an async function into a function returning the tool it defines.
///
/// See the documentation of `claude_agent_rust_sdk::tool`.
#[proc_macro_attribute]
pub fn tool(arguments: TokenStream, item: TokenStream) -> TokenStream {
    expand(arguments.into(), item.into())
        .unwrap_or_else(Error::into_compile_error)
        .into()
}

#[derive(Default)]
struct Settings {
    krate: Option<Path>,
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
                "crate" => {
                    let path: LitStr = meta.value()?.parse()?;
                    settings.krate = Some(path.parse()?);
                }
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
                         `open_world_hint`, `max_result_size_chars`, or `crate`",
                    ));
                }
            }
            Ok(())
        });
        parser.parse2(arguments)?;
        Ok(settings)
    }

    /// The path of the SDK crate in the generated code: `crate = "..."`, or
    /// `::claude_agent_rust_sdk`.
    fn krate(&self) -> TokenStream2 {
        self.krate.as_ref().map_or_else(
            || quote!(::claude_agent_rust_sdk),
            ToTokens::to_token_stream,
        )
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
        let krate = self.krate();
        Some(quote! {
            .annotations(#krate::ToolAnnotations::new() #(#setters)*)
        })
    }
}

/// The doc comment of the function, as rustdoc reads it: the lines of every
/// `#[doc]` attribute, less the indentation they all share, with blank lines
/// at either end left out. Indentation beyond the shared part stays, so an
/// indented code block or a nested list keeps its shape.
fn doc_comment(attributes: &[Attribute]) -> Option<String> {
    let text: Vec<String> = attributes
        .iter()
        .filter(|attribute| attribute.path().is_ident("doc"))
        .filter_map(|attribute| match &attribute.meta {
            Meta::NameValue(pair) => match &pair.value {
                Expr::Lit(literal) => match &literal.lit {
                    Lit::Str(text) => Some(text.value()),
                    _ => None,
                },
                _ => None,
            },
            _ => None,
        })
        .collect();
    let lines: Vec<&str> = text
        .iter()
        .flat_map(|text| text.split('\n'))
        .map(str::trim_end)
        .collect();
    let indent = lines
        .iter()
        .filter(|line| !line.is_empty())
        .map(|line| line.len() - line.trim_start().len())
        .min()
        .unwrap_or(0);
    let lines: Vec<&str> = lines
        .iter()
        .map(|line| line.get(indent..).unwrap_or_else(|| line.trim_start()))
        .collect();
    let text = lines.join("\n").trim_matches('\n').to_owned();
    (!text.trim().is_empty()).then_some(text)
}

/// How the tool function receives the state it was built with.
enum State<'a> {
    /// `state: &S`: the generated function takes an `Arc<S>`.
    Shared { name: Ident, inner: &'a Type },
    /// `state: T`: the generated function takes a `T` and clones it for each
    /// call.
    Owned { name: Ident, ty: &'a Type },
}

/// The inputs of a tool function: an optional state, then the argument
/// struct.
struct Inputs<'a> {
    state: Option<State<'a>>,
    argument: &'a Type,
}

fn typed_input<'a>(function: &'a ItemFn, input: &'a FnArg) -> Result<&'a PatType> {
    let name = &function.sig.ident;
    match input {
        FnArg::Typed(argument) => Ok(argument),
        FnArg::Receiver(receiver) => Err(Error::new_spanned(
            receiver,
            format!(
                "#[tool] cannot take `self`: `{name}` must be a free function; pass its state \
                 as a first argument instead, as in `async fn {name}(state: &State, args: Args)`"
            ),
        )),
    }
}

fn state_of(input: &PatType) -> Result<State<'_>> {
    let name = match &*input.pat {
        Pat::Ident(binding) => binding.ident.clone(),
        _ => Ident::new("state", proc_macro2::Span::call_site()),
    };
    match &*input.ty {
        Type::Reference(reference) if reference.mutability.is_some() => Err(Error::new_spanned(
            reference,
            "#[tool] cannot hand out `&mut` state, since calls may run at the same time: take \
             `&State` and keep what changes behind a lock",
        )),
        Type::Reference(reference) => Ok(State::Shared {
            name,
            inner: &reference.elem,
        }),
        ty => Ok(State::Owned { name, ty }),
    }
}

fn inputs(function: &ItemFn) -> Result<Inputs<'_>> {
    let name = &function.sig.ident;
    let inputs: Vec<&FnArg> = function.sig.inputs.iter().collect();
    match inputs[..] {
        [argument] => Ok(Inputs {
            state: None,
            argument: &typed_input(function, argument)?.ty,
        }),
        [state, argument] => {
            let state = typed_input(function, state)?;
            let argument = typed_input(function, argument)?;
            Ok(Inputs {
                state: Some(state_of(state)?),
                argument: &argument.ty,
            })
        }
        _ => {
            let found = match inputs.len() {
                0 => "no argument".to_owned(),
                count => format!("{count} arguments"),
            };
            Err(Error::new_spanned(
                &function.sig,
                format!(
                    "#[tool] expects an argument struct, after an optional state, but `{name}` \
                     takes {found}: declare it as `async fn {name}(args: Args) -> \
                     Result<ToolResult, E>` or `async fn {name}(state: &State, args: Args) -> \
                     Result<ToolResult, E>`"
                ),
            ))
        }
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
    let Inputs { state, argument } = inputs(&function)?;

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
    let krate = settings.krate();

    let attributes = &function.attrs;
    let visibility = &function.vis;
    let signature = &function.sig;
    let block = &function.block;
    let arguments = quote!(__claude_agent_rust_sdk_arguments);
    let (parameter, prepare, call) = match &state {
        None => (quote!(), quote!(), quote!(#ident(#arguments))),
        Some(State::Shared { name, inner }) => (
            quote!(#name: ::std::sync::Arc<#inner>),
            quote!(let #name = ::std::sync::Arc::clone(&#name);),
            quote!(#ident(&#name, #arguments)),
        ),
        Some(State::Owned { name, ty }) => (
            quote!(#name: #ty),
            quote!(let #name = ::std::clone::Clone::clone(&#name);),
            quote!(#ident(#name, #arguments)),
        ),
    };
    Ok(quote! {
        #(#attributes)*
        #visibility fn #ident(#parameter) -> #krate::SdkMcpTool {
            #signature #block

            #krate::SdkMcpTool::typed::<#argument>(
                #name,
                #description,
                move |#arguments| {
                    #prepare
                    async move {
                        #call
                            .await
                            .map_err(#krate::BoxError::from)
                    }
                },
            )
            #annotations
        }
    })
}
