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
//! Generates `pub fn register_all_commands(app)`.
//!
//! ```ignore
//! register_commands!(on_drive_rover, on_brake_rover);
//! ```

use proc_macro::TokenStream;
use proc_macro2::TokenStream as TokenStream2;
use quote::quote;
use syn::{
    parse_macro_input, DeriveInput, Ident, ItemFn, Token, Data, Fields,
    punctuated::Punctuated,
};

// ── #[Command] — struct attribute ─────────────────────────────────────────

/// Attribute macro that marks a struct as a typed simulation command.
///
/// Replaces the struct with one that has `Event + Reflect + Clone + Debug` derives.
///
/// Use `#[Command(default)]` to also derive `Default` for reflect-based construction:
/// ```ignore
/// #[Command(default)]  // Adds Default derive + #[reflect(Default)]
/// pub struct CaptureScreenshot {
///     pub target: Option<Entity>,
/// }
/// ```
#[proc_macro_attribute]
pub fn Command(attr: TokenStream, item: TokenStream) -> TokenStream {
    let input = parse_macro_input!(item as DeriveInput);
    let name = &input.ident;
    let vis = &input.vis;
    let generics = &input.generics;
    let (_impl_generics, _ty_generics, where_clause) = generics.split_for_impl();

    // Check for "default" keyword in attributes
    let attr_str = attr.to_string();
    let wants_default = attr_str.trim() == "default";

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

    let (derive, reflect) = if wants_default {
        (
            quote!(bevy::prelude::Event, bevy::prelude::Reflect, Clone, Debug, Default),
            quote!(#[reflect(Event, Default)]),
        )
    } else {
        (
            quote!(bevy::prelude::Event, bevy::prelude::Reflect, Clone, Debug),
            quote!(#[reflect(Event)]),
        )
    };

    let expanded = quote! {
        #[derive(#derive)]
        #reflect
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
/// 2. Generates `__register_<fn_name>(app)` that calls `register_type` + `add_observer`.
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

    let expanded = quote! {
        /// Generated registration function — call via `register_commands!`.
        #fn_vis fn #register_fn_name(app: &mut bevy::prelude::App) {
            app.register_type::<#cmd_type>();
            app.add_observer(#fn_name);
        }

        /// Observer function for `#cmd_type`.
        #fn_vis fn #fn_name(
            trigger: bevy::prelude::On<#cmd_type>,
            #(#existing_params),*
        ) {
            let cmd = trigger.event();
            #fn_body
        }
    };

    TokenStream::from(expanded)
}

// ── register_commands!() ───────────────────────────────────────────────────

/// Generates a `register_all_commands(app)` function.
#[proc_macro]
pub fn register_commands(input: TokenStream) -> TokenStream {
    let args: Punctuated<Ident, Token![,]> =
        match syn::parse::Parser::parse2(Punctuated::parse_terminated, input.into()) {
            Ok(p) => p,
            Err(e) => return e.to_compile_error().into(),
        };

    let calls: Vec<TokenStream2> = args.iter().map(|name| {
        let register_fn = Ident::new(&format!("__register_{}", name), name.span());
        quote! { #register_fn(app); }
    }).collect();

    let expanded = quote! {
        /// Registers all typed commands with the Bevy app.
        pub fn register_all_commands(app: &mut bevy::prelude::App) {
            #(#calls)*
        }
    };

    TokenStream::from(expanded)
}
