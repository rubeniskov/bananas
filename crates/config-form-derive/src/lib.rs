//! Proc-macro that turns a struct annotated with
//! `#[config_form(api = "/api/foo/config", …)]` into a complete
//! BanaNAS plugin config form. Pairs with the `bananas-config-form`
//! runtime crate which ships the row widgets, hydrate/submit
//! hooks, and the shared form-state bundle.
//!
//! Inputs (per field):
//! ```ignore
//! #[field(
//!     label = "Width (px)",
//!     tooltip = "Render width…",
//!     min = 320, max = 4096, step = 1,
//!     placeholder = "auto",
//!     select = ["auto", "dark", "light"],   // String fields → <select>
//!     csv,                                  // Vec<String> → CSV input
//!     legend = "Render target",             // start a new <fieldset>
//!     skip,                                 // round-trip via serde, hide in UI
//! )]
//! ```
//!
//! Output:
//!
//! - `impl bananas_config_form::ConfigForm for Cfg` with
//!   `ENDPOINT`/`FORM_ID`/`DESCRIPTION`/`SUCCESS_MSG` consts.
//! - `impl Cfg { pub fn view(props: ConfigFormProps) -> Element }`
//!   that creates per-field signals, hydrates from the API,
//!   composes the TOML on submit, and renders the form via the
//!   row widgets in `bananas_config_form::widgets`.

use proc_macro::TokenStream;
use proc_macro2::TokenStream as TokenStream2;
use quote::{format_ident, quote};
use syn::{
    Attribute, Data, DeriveInput, Expr, ExprLit, Fields, Lit, LitStr, Type, parse_macro_input,
    spanned::Spanned,
};

#[proc_macro_derive(ConfigForm, attributes(config_form, field))]
pub fn derive_config_form(input: TokenStream) -> TokenStream {
    let input = parse_macro_input!(input as DeriveInput);
    match expand(&input) {
        Ok(ts) => ts.into(),
        Err(e) => e.to_compile_error().into(),
    }
}

fn expand(input: &DeriveInput) -> syn::Result<TokenStream2> {
    let struct_name = &input.ident;
    let cfg = parse_struct_attrs(&input.attrs)?;
    let fields = match &input.data {
        Data::Struct(s) => match &s.fields {
            Fields::Named(named) => &named.named,
            _ => {
                return Err(syn::Error::new(
                    input.span(),
                    "ConfigForm only supports structs with named fields",
                ));
            }
        },
        _ => {
            return Err(syn::Error::new(
                input.span(),
                "ConfigForm only supports named-field structs",
            ));
        }
    };

    // Per-field metadata
    let mut entries: Vec<FieldEntry> = Vec::new();
    for f in fields.iter() {
        let attrs = parse_field_attrs(&f.attrs)?;
        let ident = f.ident.clone().expect("named field");
        let ty = f.ty.clone();
        entries.push(FieldEntry { ident, ty, attrs });
    }

    let trait_impl = emit_trait_impl(struct_name, &cfg);
    let view_impl = emit_view_method(struct_name, &cfg, &entries)?;

    Ok(quote! {
        #trait_impl
        #view_impl
    })
}

// ---------- struct-level attrs ----------------------------------------------

#[derive(Default)]
struct CfgAttrs {
    api: Option<String>,
    form_id: Option<String>,
    description: String,
    success_msg: String,
    save_label: Option<String>,
}

fn parse_struct_attrs(attrs: &[Attribute]) -> syn::Result<CfgAttrs> {
    let mut out = CfgAttrs::default();
    for a in attrs {
        if !a.path().is_ident("config_form") {
            continue;
        }
        a.parse_nested_meta(|meta| {
            let key = meta
                .path
                .get_ident()
                .ok_or_else(|| meta.error("expected key"))?
                .to_string();
            let val: LitStr = meta.value()?.parse()?;
            match key.as_str() {
                "api" => out.api = Some(val.value()),
                "form_id" => out.form_id = Some(val.value()),
                "description" => out.description = val.value(),
                "success_msg" => out.success_msg = val.value(),
                "save_label" => out.save_label = Some(val.value()),
                other => {
                    return Err(meta.error(format!("unknown config_form key `{other}`")));
                }
            }
            Ok(())
        })?;
    }
    Ok(out)
}

// ---------- field-level attrs -----------------------------------------------

#[derive(Default)]
struct FieldAttrs {
    label: Option<String>,
    tooltip: Option<String>,
    min: Option<i64>,
    max: Option<i64>,
    step: Option<i64>,
    placeholder: Option<String>,
    select: Option<Vec<(String, String)>>,
    csv: bool,
    skip: bool,
    legend: Option<String>,
    required: Option<bool>,
}

fn parse_field_attrs(attrs: &[Attribute]) -> syn::Result<FieldAttrs> {
    let mut out = FieldAttrs::default();
    for a in attrs {
        if !a.path().is_ident("field") {
            continue;
        }
        a.parse_nested_meta(|meta| {
            let key = meta
                .path
                .get_ident()
                .ok_or_else(|| meta.error("expected key"))?
                .to_string();
            match key.as_str() {
                "skip" => out.skip = true,
                "csv" => out.csv = true,
                "label" => {
                    out.label = Some(meta.value()?.parse::<LitStr>()?.value());
                }
                "tooltip" => {
                    out.tooltip = Some(meta.value()?.parse::<LitStr>()?.value());
                }
                "placeholder" => {
                    out.placeholder = Some(meta.value()?.parse::<LitStr>()?.value());
                }
                "legend" => {
                    out.legend = Some(meta.value()?.parse::<LitStr>()?.value());
                }
                "min" => {
                    out.min = Some(parse_int_lit(&meta.value()?.parse::<Expr>()?)?);
                }
                "max" => {
                    out.max = Some(parse_int_lit(&meta.value()?.parse::<Expr>()?)?);
                }
                "step" => {
                    out.step = Some(parse_int_lit(&meta.value()?.parse::<Expr>()?)?);
                }
                "required" => {
                    let expr: Expr = meta.value()?.parse()?;
                    if let Expr::Lit(ExprLit {
                        lit: Lit::Bool(b), ..
                    }) = expr
                    {
                        out.required = Some(b.value);
                    } else {
                        return Err(meta.error("required = true|false"));
                    }
                }
                "select" => {
                    // `select = ["auto", "dark"]` or
                    // `select = [("auto", "auto (clock-driven)"), …]`
                    let expr: Expr = meta.value()?.parse()?;
                    out.select = Some(parse_select_list(&expr)?);
                }
                other => {
                    return Err(meta.error(format!("unknown field key `{other}`")));
                }
            }
            Ok(())
        })?;
    }
    Ok(out)
}

fn parse_int_lit(e: &Expr) -> syn::Result<i64> {
    if let Expr::Lit(ExprLit {
        lit: Lit::Int(i), ..
    }) = e
    {
        return i.base10_parse::<i64>();
    }
    Err(syn::Error::new(e.span(), "expected an integer literal"))
}

fn parse_select_list(e: &Expr) -> syn::Result<Vec<(String, String)>> {
    let arr = match e {
        Expr::Array(a) => a,
        _ => {
            return Err(syn::Error::new(
                e.span(),
                "select = [...] expects an array literal",
            ));
        }
    };
    let mut out = Vec::with_capacity(arr.elems.len());
    for el in arr.elems.iter() {
        match el {
            Expr::Lit(ExprLit {
                lit: Lit::Str(s), ..
            }) => {
                let v = s.value();
                out.push((v.clone(), v));
            }
            Expr::Tuple(tup) if tup.elems.len() == 2 => {
                let (a, b) = (&tup.elems[0], &tup.elems[1]);
                let a = match a {
                    Expr::Lit(ExprLit {
                        lit: Lit::Str(s), ..
                    }) => s.value(),
                    _ => {
                        return Err(syn::Error::new(
                            a.span(),
                            "select tuple value must be a string literal",
                        ));
                    }
                };
                let b = match b {
                    Expr::Lit(ExprLit {
                        lit: Lit::Str(s), ..
                    }) => s.value(),
                    _ => {
                        return Err(syn::Error::new(
                            b.span(),
                            "select tuple label must be a string literal",
                        ));
                    }
                };
                out.push((a, b));
            }
            _ => {
                return Err(syn::Error::new(
                    el.span(),
                    "select element must be a string or (str, str) tuple",
                ));
            }
        }
    }
    Ok(out)
}

// ---------- Per-field model + type analysis ---------------------------------

struct FieldEntry {
    ident: syn::Ident,
    ty: Type,
    attrs: FieldAttrs,
}

#[derive(Clone, Copy, PartialEq)]
enum FieldKind {
    Number,
    Text,
    Csv,
    Select,
    Skip,
}

fn classify(entry: &FieldEntry) -> FieldKind {
    if entry.attrs.skip {
        return FieldKind::Skip;
    }
    if entry.attrs.select.is_some() {
        return FieldKind::Select;
    }
    if entry.attrs.csv || is_vec_of_string(&entry.ty) {
        return FieldKind::Csv;
    }
    if is_numeric(&entry.ty) {
        return FieldKind::Number;
    }
    FieldKind::Text
}

fn type_path_str(ty: &Type) -> String {
    quote!(#ty).to_string().replace(' ', "")
}

fn is_numeric(ty: &Type) -> bool {
    let s = type_path_str(ty);
    let bare = strip_option(&s).unwrap_or(&s);
    matches!(
        bare,
        "u8" | "u16"
            | "u32"
            | "u64"
            | "u128"
            | "usize"
            | "i8"
            | "i16"
            | "i32"
            | "i64"
            | "i128"
            | "isize"
            | "f32"
            | "f64"
    )
}

fn is_vec_of_string(ty: &Type) -> bool {
    let s = type_path_str(ty);
    s == "Vec<String>" || s == "std::vec::Vec<String>"
}

fn strip_option(s: &str) -> Option<&str> {
    s.strip_prefix("Option<").and_then(|s| s.strip_suffix('>'))
}

fn is_optional(ty: &Type) -> bool {
    strip_option(&type_path_str(ty)).is_some()
}

// ---------- Code emit -------------------------------------------------------

fn emit_trait_impl(struct_name: &syn::Ident, cfg: &CfgAttrs) -> TokenStream2 {
    let endpoint = cfg.api.as_deref().unwrap_or("");
    let form_id = cfg.form_id.as_deref().unwrap_or_else(|| {
        // Default to the snake-cased struct name + "-form".
        Box::leak(format!("{}-form", to_kebab(&struct_name.to_string())).into_boxed_str())
    });
    let desc = &cfg.description;
    let success = &cfg.success_msg;

    if endpoint.is_empty() {
        // Sub-section: no top-level impl. Sub-sections aren't yet
        // supported; the macro only handles top-level forms in this
        // first cut. Surface a clear compile error.
        return quote! {
            compile_error!(
                "ConfigForm currently only supports top-level forms. \
                 Add #[config_form(api = \"/api/.../config\")] to the struct."
            );
        };
    }

    quote! {
        impl ::bananas_config_form::ConfigForm for #struct_name {
            const ENDPOINT: &'static str = #endpoint;
            const FORM_ID: &'static str = #form_id;
            const DESCRIPTION: &'static str = #desc;
            const SUCCESS_MSG: &'static str = #success;
        }
    }
}

fn emit_view_method(
    struct_name: &syn::Ident,
    cfg: &CfgAttrs,
    entries: &[FieldEntry],
) -> syn::Result<TokenStream2> {
    let save_label = cfg.save_label.as_deref().unwrap_or("Save");

    // Per-field signals + hydrate + compose tokens, accumulated.
    let mut signal_decls: Vec<TokenStream2> = Vec::new();
    let mut hydrate_assigns: Vec<TokenStream2> = Vec::new();
    let mut compose_assigns: Vec<TokenStream2> = Vec::new();
    let mut row_renders: Vec<TokenStream2> = Vec::new();

    // Sections: track the current legend and emit a fresh
    // <fieldset> when a field carries `legend = "…"`.
    let mut current_legend: Option<&str> = None;
    let mut section_buffer: Vec<TokenStream2> = Vec::new();
    let mut sections: Vec<(String, Vec<TokenStream2>)> = Vec::new();

    for entry in entries {
        let kind = classify(entry);
        let ident = &entry.ident;
        let sig_name = format_ident!("__sig_{}", ident);

        // Skipped fields: still need a default-clone in the signal
        // bundle so `compose` can reproduce them on submit.
        if kind == FieldKind::Skip {
            signal_decls.push(quote! {
                let mut #sig_name = ::dioxus::prelude::use_signal({
                    let __default: #struct_name = <#struct_name as ::std::default::Default>::default();
                    let __v = __default.#ident.clone();
                    move || __v.clone()
                });
            });
            hydrate_assigns.push(quote! {
                #sig_name.set(__loaded.#ident.clone());
            });
            compose_assigns.push(quote! {
                #ident: #sig_name.read().clone(),
            });
            continue;
        }

        // Visible fields: signals are Signal<String>.
        let default_lit = default_string_for(&entry.ty);
        signal_decls.push(quote! {
            let mut #sig_name = ::dioxus::prelude::use_signal(|| #default_lit.to_string());
        });

        let ty_str = type_path_str(&entry.ty);
        let optional = is_optional(&entry.ty);
        hydrate_assigns.push(emit_hydrate_for(&kind, ident, &sig_name, &ty_str, optional));
        compose_assigns.push(emit_compose_for(
            &kind, ident, &sig_name, &entry.ty, optional,
        ));

        // Move the previous section into `sections` if a new
        // `legend` opens here.
        if let Some(leg) = entry.attrs.legend.as_deref() {
            if !section_buffer.is_empty()
                && let Some(prev) = current_legend.take()
            {
                sections.push((prev.to_string(), std::mem::take(&mut section_buffer)));
            }
            current_legend = Some(Box::leak(leg.to_string().into_boxed_str()));
        }
        if current_legend.is_none() {
            // No section opened yet — assume the whole struct lives
            // in a single fieldset legend == the struct's display
            // name. The dashboard form uses two fieldsets, the stats
            // form uses 4. Both annotate the first field of each
            // section; we treat an unannotated leading field as an
            // error so we don't lose track of the layout.
            return Err(syn::Error::new(
                entry.ident.span(),
                "first non-skipped field must carry `#[field(legend = \"…\")]` to open a section",
            ));
        }

        let row = emit_row(&kind, &entry.attrs, ident, &sig_name);
        section_buffer.push(row);
    }
    // Flush trailing section.
    if !section_buffer.is_empty()
        && let Some(prev) = current_legend
    {
        sections.push((prev.to_string(), section_buffer));
    }

    let section_renders: Vec<TokenStream2> = sections
        .into_iter()
        .map(|(legend, rows)| {
            quote! {
                fieldset {
                    legend { #legend }
                    #(#rows)*
                }
            }
        })
        .collect();
    row_renders.extend(section_renders);

    // Stable ids for the TOML preview textarea.
    let toml_id_str = format!("{}-toml", cfg.form_id.as_deref().unwrap_or("config"));

    Ok(quote! {
        impl #struct_name {
            #[allow(non_snake_case)]
            pub fn view(
                props: ::bananas_config_form::ConfigFormProps,
            ) -> ::dioxus::prelude::Element {
                use ::dioxus::prelude::*;
                use ::bananas_config_form::{
                    ConfigForm as _,
                    NumberRow, TextRow, SelectRow, CsvRow, SaveBar, TextareaWithCopy,
                };

                #(#signal_decls)*

                let on_unauthorized = props.on_unauthorized;
                let inline_save = props.inline_save;

                let on_loaded = move |toml_str: String| {
                    let __loaded: #struct_name = ::toml::from_str(&toml_str).unwrap_or_default();
                    #(#hydrate_assigns)*
                };
                let unauth = move || on_unauthorized.call(());
                let state = ::bananas_config_form::use_form_state::<#struct_name>(on_loaded, unauth);

                let compose_toml = move || -> String {
                    let __new: #struct_name = #struct_name {
                        #(#compose_assigns)*
                    };
                    ::toml::to_string_pretty(&__new).unwrap_or_default()
                };

                let preview = compose_toml();
                let form_id: &'static str = <#struct_name as ::bananas_config_form::ConfigForm>::FORM_ID;
                let description: &'static str = <#struct_name as ::bananas_config_form::ConfigForm>::DESCRIPTION;
                let endpoint: &'static str = <#struct_name as ::bananas_config_form::ConfigForm>::ENDPOINT;

                let mut submit_state = state;
                let on_submit = move |e: ::dioxus::prelude::FormEvent| {
                    e.prevent_default();
                    let toml_text = compose_toml();
                    let unauth = move || on_unauthorized.call(());
                    spawn(async move {
                        ::bananas_config_form::__private_submit::<#struct_name>(
                            toml_text,
                            submit_state,
                            unauth,
                        ).await;
                    });
                    let _ = &mut submit_state;
                };

                let mut error_sig = state.error;
                let mut info_sig = state.info;
                let mut hydrated_sig = state.hydrated;
                let mut load_failed_sig = state.load_failed;
                let busy_sig = state.busy;
                let hydr2 = state.hydrated;

                rsx! {
                    form {
                        id: "{form_id}",
                        class: "settings-form",
                        onsubmit: on_submit,
                        p { class: "preview-label", "{description}" }
                        if let Some(msg) = error_sig() {
                            div { class: "banner err", pre { "{msg}" } }
                        }
                        if let Some(msg) = info_sig() {
                            div { class: "banner ok", pre { "{msg}" } }
                        }
                        if !hydrated_sig() && !load_failed_sig() {
                            p { class: "preview-label", "Loading…" }
                        }
                        #(#row_renders)*
                        if inline_save {
                            SaveBar {
                                busy: busy_sig,
                                hydrated: hydr2,
                                label: #save_label,
                            }
                        }
                        h4 { style: "margin: 1.2em 0 .4em", "Generated TOML" }
                        p { class: "preview-label",
                            "Read-only — exactly what we'll PUT to "
                            code { "{endpoint}" }
                            " on save."
                        }
                        TextareaWithCopy { value: preview, id: #toml_id_str }
                    }
                }
            }
        }
    })
}

fn emit_row(
    kind: &FieldKind,
    attrs: &FieldAttrs,
    ident: &syn::Ident,
    sig: &syn::Ident,
) -> TokenStream2 {
    let label = attrs
        .label
        .clone()
        .unwrap_or_else(|| humanize(&ident.to_string()));
    let label_lit = LitStr::new(&label, ident.span());
    let tooltip_lit = LitStr::new(attrs.tooltip.as_deref().unwrap_or(""), ident.span());
    let placeholder_lit = LitStr::new(attrs.placeholder.as_deref().unwrap_or(""), ident.span());
    let required = attrs.required.unwrap_or(true);

    match kind {
        FieldKind::Number => {
            let min = attrs
                .min
                .map(|n| quote!({ ::std::option::Option::Some(#n) }))
                .unwrap_or_else(|| quote!({ ::std::option::Option::None }));
            let max = attrs
                .max
                .map(|n| quote!({ ::std::option::Option::Some(#n) }))
                .unwrap_or_else(|| quote!({ ::std::option::Option::None }));
            let step = attrs
                .step
                .map(|n| quote!({ ::std::option::Option::Some(#n) }))
                .unwrap_or_else(|| quote!({ ::std::option::Option::None }));
            quote! {
                NumberRow {
                    label: #label_lit,
                    tooltip: #tooltip_lit,
                    min: #min,
                    max: #max,
                    step: #step,
                    placeholder: #placeholder_lit,
                    required: #required,
                    value: #sig,
                }
            }
        }
        FieldKind::Text => quote! {
            TextRow {
                label: #label_lit,
                tooltip: #tooltip_lit,
                placeholder: #placeholder_lit,
                required: #required,
                value: #sig,
            }
        },
        FieldKind::Csv => quote! {
            CsvRow {
                label: #label_lit,
                tooltip: #tooltip_lit,
                placeholder: #placeholder_lit,
                value: #sig,
            }
        },
        FieldKind::Select => {
            let opts = attrs.select.as_ref().expect("Select kind has options");
            let pairs: Vec<TokenStream2> = opts
                .iter()
                .map(|(v, l)| {
                    let v = LitStr::new(v, ident.span());
                    let l = LitStr::new(l, ident.span());
                    quote!((#v, #l))
                })
                .collect();
            quote! {
                SelectRow {
                    label: #label_lit,
                    tooltip: #tooltip_lit,
                    options: { &[#(#pairs),*] },
                    value: #sig,
                }
            }
        }
        FieldKind::Skip => quote!(),
    }
}

fn emit_hydrate_for(
    kind: &FieldKind,
    ident: &syn::Ident,
    sig: &syn::Ident,
    ty_str: &str,
    optional: bool,
) -> TokenStream2 {
    match kind {
        FieldKind::Number | FieldKind::Text | FieldKind::Select => {
            if optional {
                quote! {
                    #sig.set(
                        match __loaded.#ident.as_ref() {
                            ::std::option::Option::Some(v) => v.to_string(),
                            ::std::option::Option::None => ::std::string::String::new(),
                        }
                    );
                }
            } else if ty_str == "String" {
                quote! { #sig.set(__loaded.#ident.clone()); }
            } else {
                quote! { #sig.set(__loaded.#ident.to_string()); }
            }
        }
        FieldKind::Csv => quote! {
            #sig.set(__loaded.#ident.join(", "));
        },
        FieldKind::Skip => quote!(),
    }
}

fn emit_compose_for(
    kind: &FieldKind,
    ident: &syn::Ident,
    sig: &syn::Ident,
    ty: &Type,
    optional: bool,
) -> TokenStream2 {
    let ty_str = type_path_str(ty);
    match kind {
        FieldKind::Number => {
            let inner = strip_option(&ty_str).unwrap_or(&ty_str);
            let inner_ty: Type = syn::parse_str(inner).expect("numeric type parses");
            if optional {
                quote! {
                    #ident: {
                        let s = #sig.read().clone();
                        let s = s.trim();
                        if s.is_empty() {
                            ::std::option::Option::None
                        } else {
                            ::std::option::Option::Some(s.parse::<#inner_ty>().unwrap_or_default())
                        }
                    },
                }
            } else {
                quote! {
                    #ident: #sig.read().parse::<#inner_ty>().unwrap_or_default(),
                }
            }
        }
        FieldKind::Text | FieldKind::Select => {
            if optional {
                quote! {
                    #ident: {
                        let s = #sig.read().clone();
                        if s.is_empty() {
                            ::std::option::Option::None
                        } else {
                            ::std::option::Option::Some(s)
                        }
                    },
                }
            } else {
                quote! { #ident: #sig.read().clone(), }
            }
        }
        FieldKind::Csv => quote! {
            #ident: #sig
                .read()
                .split(',')
                .map(|t| t.trim().to_string())
                .filter(|t| !t.is_empty())
                .collect(),
        },
        FieldKind::Skip => quote! {
            #ident: #sig.read().clone(),
        },
    }
}

// ---------- helpers ---------------------------------------------------------

fn humanize(s: &str) -> String {
    // snake_case → "Snake Case"
    let mut out = String::new();
    let mut up = true;
    for c in s.chars() {
        if c == '_' {
            out.push(' ');
            up = true;
        } else if up {
            out.push(c.to_ascii_uppercase());
            up = false;
        } else {
            out.push(c);
        }
    }
    out
}

fn to_kebab(s: &str) -> String {
    let mut out = String::with_capacity(s.len());
    for c in s.chars() {
        if c.is_ascii_uppercase() && !out.is_empty() {
            out.push('-');
            out.push(c.to_ascii_lowercase());
        } else if c == '_' {
            out.push('-');
        } else {
            out.push(c.to_ascii_lowercase());
        }
    }
    out
}

fn default_string_for(ty: &Type) -> &'static str {
    let s = type_path_str(ty);
    if is_numeric(ty) {
        if s.starts_with("Option<") {
            return "";
        }
        // Use a sane numeric default
        return "0";
    }
    if s.starts_with("Option<") || s == "String" {
        return "";
    }
    if s == "Vec<String>" || s == "std::vec::Vec<String>" {
        return "";
    }
    ""
}
