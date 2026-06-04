//! Proc-macros for LunCoSim's typed command system.
//!
//! # On a struct: `#[Command]`
//!
//! Marks a struct as a typed simulation command.
//! Replaces `Event + Reflect + Clone + Debug`.
//!
//! ```ignore
//! #[Command]
//! pub struct DriveRover {
//!     pub target: Entity,
//!     pub forward: f64,
//!     pub steer: f64,
//! }
//! ```
//!
//! # On a function: `#[on_command(TypeName)]`
//!
//! Wraps the observer and generates a registration helper.
//!
//! ```ignore
//! #[on_command(DriveRover)]
//! fn on_drive_rover(cmd: DriveRover, mut q: Query<&mut Fsw>) { ... }
//! ```
//!
//! # Registration: `register_commands!(fn_a, fn_b)`
//!
//! Generates `pub fn register_all_commands(app)`. Entries may be bare
//! idents (same module) or module paths — the path form lets observers
//! live in split submodules without per-fn `use` shims.
//!
//! ```ignore
//! register_commands!(on_drive_rover, on_brake_rover);
//! register_commands!(nav::on_set_zoom, doc::on_undo);
//! ```

use proc_macro::TokenStream;
use proc_macro2::TokenStream as TokenStream2;
use quote::quote;
use syn::{
    parse_macro_input, parse_quote, DeriveInput, Field, Ident, ItemFn, Path, Token, Data, Fields,
    punctuated::Punctuated,
};

// ── #[Command] — struct attribute ─────────────────────────────────────────

/// Attribute macro that marks a struct as a typed simulation command.
///
/// Always derives:
/// - `bevy::Event` — dispatchable on the Bevy command bus.
/// - `bevy::Reflect` + `#[reflect(Event)]` — ad-hoc dispatch from the
///   HTTP API's reflection-based deserializer.
/// - `Clone, Debug`.
/// - `serde::Serialize, serde::Deserialize` — typed roundtrip for
///   journals, network sync, CLI clients, AI-agent flows. Requires the
///   workspace's bevy `serialize` feature so `Entity`, `Vec3`, `Handle`
///   etc. cooperate.
///
/// Optional keyword:
///
/// - `default` — also derive `Default` and emit `#[serde(default)]` so
///   JSON with omitted fields fills in defaults.
///   ```ignore
///   #[Command(default)]
///   pub struct OpenFile { pub path: String, pub doc: DocumentId }
///   ```
#[proc_macro_attribute]
// Why: PascalCase mimics `#[derive(Trait)]` syntax at the call site
// (`#[Command]` reads as "make this struct a Command"). Renaming to
// `command` would compile but lose the visual cue that `#[Command]`
// produces a Command type — the same convention bevy uses for `#[derive(Event)]`.
#[allow(non_snake_case)]
pub fn Command(attr: TokenStream, item: TokenStream) -> TokenStream {
    let input = parse_macro_input!(item as DeriveInput);
    let name = &input.ident;
    let vis = &input.vis;
    let generics = &input.generics;
    let (_impl_generics, _ty_generics, where_clause) = generics.split_for_impl();

    // Parse comma-separated keywords from the attribute.
    let attr_str = attr.to_string();
    let keywords: std::collections::HashSet<&str> = attr_str
        .split(',')
        .map(str::trim)
        .filter(|s| !s.is_empty())
        .collect();
    let wants_default = keywords.contains("default");

    // Reject unknown keywords so typos don't silently no-op.
    // `serde` was previously opt-in; it's now always on. Accept it as
    // a no-op for one release to avoid breaking callers that already
    // wrote `#[Command(default, serde)]`.
    for kw in &keywords {
        if !matches!(*kw, "default" | "serde") {
            return syn::Error::new_spanned(
                &input,
                format!("unknown #[Command] keyword: `{}` (expected `default`)", kw),
            )
            .to_compile_error()
            .into();
        }
    }

    let fields = match &input.data {
        Data::Struct(ds) => match &ds.fields {
            Fields::Named(n) => &n.named,
            _ => return syn::Error::new_spanned(&input,
                "Command requires named fields, e.g. `pub struct Foo { bar: u32 }`")
                .to_compile_error().into(),
        },
        _ => return syn::Error::new_spanned(&input, "Command can only be used on structs")
            .to_compile_error().into(),
    };

    // Rewrite field-role sugar into bevy reflect custom attributes, consumed
    // here (they never reach rustc as real attributes); everything else on the
    // field is forwarded untouched.
    //   `#[wire_local]`   → local-only Entity: the wire codec substitutes
    //                       `Entity::PLACEHOLDER` instead of leaking local bits.
    //   `#[authz_target]` → the gid the host authorizes ownership against.
    // The codec/apply paths read these via `NamedField::has_attribute::<_>()`.
    let fields: Punctuated<Field, Token![,]> = fields
        .iter()
        .cloned()
        .map(|mut f| {
            if f.attrs.iter().any(|a| a.path().is_ident("wire_local")) {
                f.attrs.retain(|a| !a.path().is_ident("wire_local"));
                f.attrs.push(parse_quote!(#[reflect(@::lunco_core::WireLocal)]));
            }
            if f.attrs.iter().any(|a| a.path().is_ident("authz_target")) {
                f.attrs.retain(|a| !a.path().is_ident("authz_target"));
                f.attrs.push(parse_quote!(#[reflect(@::lunco_core::AuthzTarget)]));
            }
            f
        })
        .collect();

    let mut derives: Vec<TokenStream2> = vec![
        quote!(bevy::prelude::Event),
        quote!(bevy::prelude::Reflect),
        quote!(Clone),
        quote!(Debug),
        // Fully-qualified through lunco-core's re-export so callers
        // don't need their own `serde` dependency.
        quote!(::lunco_core::serde::Serialize),
        quote!(::lunco_core::serde::Deserialize),
    ];
    if wants_default {
        derives.push(quote!(Default));
    }

    let reflect = if wants_default {
        quote!(#[reflect(Event, Default)])
    } else {
        quote!(#[reflect(Event)])
    };

    // `#[serde(crate = "...")]` tells the serde derive where to find
    // the `serde` crate items it generates references to. Required
    // because we route through lunco-core's re-export instead of a
    // direct `serde` dependency at every call site.
    let serde_crate_attr = quote!(#[serde(crate = "::lunco_core::serde")]);
    let serde_default_attr = if wants_default {
        quote!(#[serde(default)])
    } else {
        quote!()
    };

    // Forward the input struct's outer attributes (notably `#[doc = "..."]`
    // comments) so rustdoc and `missing_docs` see the user-written docs.
    let attrs = &input.attrs;

    let expanded = quote! {
        #(#attrs)*
        // `Reflect` (and other derives) generate associated impl helpers
        // that satisfy the public-API trait surface but are not user-
        // facing items worth documenting individually.
        #[allow(missing_docs)]
        #[derive(#(#derives),*)]
        #reflect
        #serde_crate_attr
        #serde_default_attr
        #vis struct #name #generics #where_clause {
            #fields
        }
    };

    TokenStream::from(expanded)
}

// ── #[on_command(TypeName)] — function attribute ──────────────────────────

/// Annotates an observer function for a typed command.
///
/// 1. Wraps the function to accept `On<T>` as the first parameter.
/// 2. Generates a `__register_<fn_name>(app)` helper (`register_type` +
///    `add_observer`). Don't call it by hand — list the observer in a
///    [`register_commands!`] invocation (bare or `module::fn` path) and
///    let the generated `register_all_commands(app)` wire it up.
#[proc_macro_attribute]
pub fn on_command(attr: TokenStream, item: TokenStream) -> TokenStream {
    let cmd_type: Ident = match syn::parse(attr) {
        Ok(id) => id,
        Err(e) => return e.to_compile_error().into(),
    };
    let func = parse_macro_input!(item as ItemFn);
    let fn_name = &func.sig.ident;
    let fn_vis = &func.vis;
    let fn_body = &func.block;

    let existing_params: Vec<_> = func.sig.inputs.iter().skip(1).collect();

    let register_fn_name = Ident::new(
        &format!("__register_{}", fn_name),
        fn_name.span(),
    );

    // A handler with a return type (`-> Result<Ack, String>`) opts into
    // result recording: the wrapper runs the body, then — if a transport
    // set the active request id — records the outcome in `CommandResults`.
    // Void handlers (the common, fire-and-forget case) keep the lean
    // passthrough wrapper with no extra params or resource access.
    let returns_result = !matches!(func.sig.output, syn::ReturnType::Default);

    let observer_fn = if returns_result {
        quote! {
            /// Observer function for `#cmd_type` (records its outcome).
            #fn_vis fn #fn_name(
                trigger: bevy::prelude::On<#cmd_type>,
                #(#existing_params,)*
                mut __lunco_cmd_results: bevy::prelude::ResMut<::lunco_core::CommandResults>,
                __lunco_active_id: bevy::prelude::Res<::lunco_core::ActiveCommandId>,
            ) {
                let cmd = trigger.event();
                let __lunco_outcome: ::core::result::Result<::lunco_core::Ack, ::std::string::String> =
                    (|| #fn_body)();
                if let Some(__id) = __lunco_active_id.get() {
                    __lunco_cmd_results.record(__id, __lunco_outcome);
                }
            }
        }
    } else {
        quote! {
            /// Observer function for `#cmd_type`.
            #fn_vis fn #fn_name(
                trigger: bevy::prelude::On<#cmd_type>,
                #(#existing_params),*
            ) {
                let cmd = trigger.event();
                #fn_body
            }
        }
    };

    let expanded = quote! {
        /// Generated registration function — call via `register_commands!`.
        #fn_vis fn #register_fn_name(app: &mut bevy::prelude::App) {
            app.register_type::<#cmd_type>();
            app.add_observer(#fn_name);
        }

        #observer_fn
    };

    TokenStream::from(expanded)
}

// ── register_commands!() ───────────────────────────────────────────────────

/// Generates a `register_all_commands(app)` function.
///
/// Accepts bare idents (`on_ping`) or module paths
/// (`lifecycle::on_open`) — the latter lets observers live in split
/// submodules and still be listed in one place. Each entry's final
/// segment is rewritten to its generated `__register_<name>` helper,
/// preserving any module prefix.
#[proc_macro]
pub fn register_commands(input: TokenStream) -> TokenStream {
    let args: Punctuated<Path, Token![,]> =
        match syn::parse::Parser::parse2(Punctuated::parse_terminated, input.into()) {
            Ok(p) => p,
            Err(e) => return e.to_compile_error().into(),
        };

    let calls: Vec<TokenStream2> = args.iter().map(|path| {
        // Rebuild the path with its final segment renamed to the
        // generated `__register_<name>` helper, keeping the module
        // prefix (e.g. `lifecycle::on_open` -> `lifecycle::__register_on_open`).
        let mut path = path.clone();
        if let Some(last) = path.segments.last_mut() {
            last.ident = Ident::new(&format!("__register_{}", last.ident), last.ident.span());
        }
        quote! { #path(app); }
    }).collect();

    let expanded = quote! {
        /// Registers all typed commands with the Bevy app.
        pub fn register_all_commands(app: &mut bevy::prelude::App) {
            #(#calls)*
        }
    };

    TokenStream::from(expanded)
}
