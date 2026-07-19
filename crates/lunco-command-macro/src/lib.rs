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

// TODO(backlog): this crate is structurally untestable — all logic lives inline in
// `proc_macro::TokenStream` entry points, which cannot be called from unit tests.
// Unlock via a `trybuild` dev-dependency (compile-pass/fail fixtures) or by
// extracting the expansion logic into proc_macro2-typed helpers testable directly.
// See the engineering-backlog doc in docs/architecture (command-macro testability).
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
    // `reflect_default`: register `ReflectDefault` (so the API reflect
    // deserializer fills MISSING fields from `Default`) WITHOUT deriving
    // `Default` — for commands whose fields aren't all `Default` (e.g. an
    // `Entity` field) but which still want optional/defaulted params. The caller
    // provides a manual `impl Default`.
    let wants_reflect_default = keywords.contains("reflect_default");

    // Reject unknown keywords so typos don't silently no-op.
    // `serde` was previously opt-in and then a grace-period no-op; it's
    // now always on, so a stale `#[Command(serde)]` fails loudly with
    // the fix spelled out.
    if keywords.contains("serde") {
        return syn::Error::new_spanned(
            &input,
            "the `serde` keyword is retired: #[Command] always derives \
             Serialize/Deserialize — remove `serde` from the attribute",
        )
        .to_compile_error()
        .into();
    }
    for kw in &keywords {
        if !matches!(*kw, "default" | "reflect_default") {
            return syn::Error::new_spanned(
                &input,
                format!(
                    "unknown #[Command] keyword: `{}` (expected `default` or `reflect_default`)",
                    kw
                ),
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
    //   `#[sync_local]`   → local-only Entity: the wire codec substitutes
    //                       `Entity::PLACEHOLDER` instead of leaking local bits.
    //   `#[authz_target]` → the gid the host authorizes ownership against.
    // The codec/apply paths read these via `NamedField::has_attribute::<_>()`.
    let fields: Punctuated<Field, Token![,]> = fields
        .iter()
        .cloned()
        .map(|mut f| {
            if f.attrs.iter().any(|a| a.path().is_ident("sync_local")) {
                f.attrs.retain(|a| !a.path().is_ident("sync_local"));
                f.attrs.push(parse_quote!(#[reflect(@::lunco_core::SyncLocal)]));
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

    // Register `ReflectDefault` when either deriving Default (`default`) or the
    // caller supplies a manual one (`reflect_default`).
    let reflect = if wants_default || wants_reflect_default {
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
/// Write the handler as `fn on_x(trigger: On<T>, /* system params */)`. The first
/// parameter must be the trigger: it is re-emitted as a canonical `On<T>` (so the
/// handler's event type can never disagree with the `T` named here), and `cmd`
/// is bound to `trigger.event()` for you. Any other first parameter is a
/// compile error — it would otherwise be dropped on the floor.
///
/// 1. Rewrites the function's first parameter to `On<T>`.
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

    // Dropping the author's first parameter (the `skip(1)` below) is INTENTIONAL,
    // not an oversight: we re-emit it ourselves as a canonical
    // `trigger: On<#cmd_type>`. That substitution is the whole point — it is what
    // makes the handler's event type structurally incapable of disagreeing with
    // the `#cmd_type` named in the attribute, so `#[on_command(SpawnEntity)]` can
    // never end up observing `MoveEntity`. The author's own annotation is
    // redundant by construction, so it is discarded rather than trusted.
    //
    // The catch: that reasoning only holds if the first parameter really IS the
    // trigger. If someone writes `#[on_command(X)] fn h(mut q: Query<..>)`, the
    // `Query` is what gets thrown away — a genuine system param silently vanishing.
    // So the convention is checked, not assumed, and violations are reported here
    // at the call site instead of erupting as an inscrutable type error inside the
    // expansion. (This check caught two long-standing offenders when it landed.)
    let first_is_trigger = match func.sig.inputs.first() {
        Some(syn::FnArg::Typed(pat)) => match &*pat.ty {
            syn::Type::Path(tp) => tp
                .path
                .segments
                .last()
                .is_some_and(|seg| seg.ident == "On"),
            _ => false,
        },
        _ => false,
    };
    if !first_is_trigger {
        return syn::Error::new_spanned(
            &func.sig,
            format!(
                "#[on_command({cmd})] expects the FIRST parameter to be the trigger, \
                 `trigger: On<{cmd}>` — it is replaced with one, so any other first \
                 parameter would be silently dropped. Add it, then take system params \
                 after it.",
                cmd = cmd_type,
            ),
        )
        .to_compile_error()
        .into();
    }

    // `skip(1)` — deliberately drop the trigger the author wrote; the quote! blocks
    // below re-add it as `trigger: On<#cmd_type>`. Everything AFTER it is passed
    // through untouched, so a handler takes system params exactly like any observer.
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
