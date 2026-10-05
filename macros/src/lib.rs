use proc_macro::TokenStream;
use proc_macro2::{Ident, Span, TokenStream as TokenStream2};
use quote::quote;
use syn::{
    Data, DeriveInput, Expr, Fields, Item, ItemFn, LitInt, LitStr, Macro, Stmt, Token,
    parse::{Parse, ParseStream},
    parse_macro_input,
    visit_mut::{self, VisitMut},
};

#[derive(Default)]
struct MeasureArgs {
    name: Option<LitStr>,
    scope: Option<Expr>,
    set: Option<LitStr>,
    histogram: Option<LitStr>,
    precision: Option<LitInt>,
    storage: Option<LitStr>,
}

#[derive(Default)]
struct RecordArgs {
    site: MeasureArgs,
    cycles: Option<Expr>,
}

struct CheckpointArgs {
    name: LitStr,
}

impl Parse for CheckpointArgs {
    fn parse(input: ParseStream<'_>) -> syn::Result<Self> {
        let name = input.parse()?;
        if input.peek(Token![,]) {
            input.parse::<Token![,]>()?;
        }
        if !input.is_empty() {
            return Err(input.error("checkpoint! accepts one metric name string"));
        }
        Ok(Self { name })
    }
}

#[derive(Clone, Copy)]
enum StorageKind {
    Atomic,
    Tls,
}

#[proc_macro_derive(MetricSet, attributes(brap))]
pub fn derive_metric_set(input: TokenStream) -> TokenStream {
    let input = parse_macro_input!(input as DeriveInput);
    expand_metric_set(input)
        .unwrap_or_else(syn::Error::into_compile_error)
        .into()
}

#[proc_macro_derive(MetricKey, attributes(brap))]
pub fn derive_metric_key(input: TokenStream) -> TokenStream {
    let input = parse_macro_input!(input as DeriveInput);
    expand_metric_key(input)
        .unwrap_or_else(syn::Error::into_compile_error)
        .into()
}

fn expand_metric_key(input: DeriveInput) -> syn::Result<TokenStream2> {
    let mut scope = None;
    for attribute in &input.attrs {
        if !attribute.path().is_ident("brap") {
            continue;
        }
        attribute.parse_nested_meta(|meta| {
            if meta.path.is_ident("scope") {
                scope = Some(meta.value()?.parse::<Expr>()?);
                return Ok(());
            }
            Err(meta.error("MetricKey supports only `scope` on the enum"))
        })?;
    }
    let scope = scope.ok_or_else(|| {
        syn::Error::new_spanned(
            &input.ident,
            "MetricKey requires #[brap(scope = \"scope-name\")]",
        )
    })?;

    let Data::Enum(data) = &input.data else {
        return Err(syn::Error::new_spanned(
            &input.ident,
            "MetricKey can only be derived for an enum",
        ));
    };
    let mut indexes = Vec::with_capacity(data.variants.len());
    let mut names = Vec::with_capacity(data.variants.len());
    for (index, variant) in data.variants.iter().enumerate() {
        if !matches!(variant.fields, Fields::Unit) {
            return Err(syn::Error::new_spanned(
                &variant.ident,
                "MetricKey variants must not have fields",
            ));
        }
        let ident = &variant.ident;
        let mut name = LitStr::new(&to_snake_case(&ident.to_string()), ident.span());
        for attribute in &variant.attrs {
            if !attribute.path().is_ident("brap") {
                continue;
            }
            attribute.parse_nested_meta(|meta| {
                if meta.path.is_ident("name") {
                    name = meta.value()?.parse()?;
                    return Ok(());
                }
                Err(meta.error("MetricKey variants support only `name`"))
            })?;
        }
        indexes.push(quote!(Self::#ident => #index));
        names.push(name);
    }

    let count = data.variants.len();
    let ident = &input.ident;
    let generics = &input.generics;
    let (impl_generics, type_generics, where_clause) = generics.split_for_impl();
    Ok(quote! {
        impl #impl_generics ::brap::metric::Key for #ident #type_generics #where_clause {
            const SCOPE: &'static str = #scope;
            const COUNT: usize = #count;

            #[inline(always)]
            fn index(self) -> usize {
                match self {
                    #(#indexes,)*
                }
            }

            fn name(index: usize) -> &'static str {
                const NAMES: [&str; #count] = [#(#names),*];
                NAMES[index]
            }
        }
    })
}

fn to_snake_case(name: &str) -> String {
    let mut result = String::with_capacity(name.len());
    for (index, character) in name.chars().enumerate() {
        if character.is_uppercase() {
            if index != 0 {
                result.push('_');
            }
            result.extend(character.to_lowercase());
        } else {
            result.push(character);
        }
    }
    result
}

fn expand_metric_set(input: DeriveInput) -> syn::Result<TokenStream2> {
    let mut scope = None;
    for attribute in &input.attrs {
        if !attribute.path().is_ident("brap") {
            continue;
        }
        attribute.parse_nested_meta(|meta| {
            if meta.path.is_ident("scope") {
                scope = Some(meta.value()?.parse::<Expr>()?);
                return Ok(());
            }
            Err(meta.error("MetricSet supports only `scope` on the struct"))
        })?;
    }
    let scope = scope.ok_or_else(|| {
        syn::Error::new_spanned(
            &input.ident,
            "MetricSet requires #[brap(scope = \"scope-name\")]",
        )
    })?;

    let Data::Struct(data) = &input.data else {
        return Err(syn::Error::new_spanned(
            &input.ident,
            "MetricSet can only be derived for a struct",
        ));
    };
    let Fields::Named(fields) = &data.fields else {
        return Err(syn::Error::new_spanned(
            &input.ident,
            "MetricSet requires named fields",
        ));
    };

    let visits = fields
        .named
        .iter()
        .map(|field| {
            let field_ident = field.ident.as_ref().expect("named field");
            let mut metric_name = LitStr::new(&field_ident.to_string(), field_ident.span());
            for attribute in &field.attrs {
                if !attribute.path().is_ident("brap") {
                    continue;
                }
                attribute.parse_nested_meta(|meta| {
                    if meta.path.is_ident("name") {
                        metric_name = meta.value()?.parse()?;
                        return Ok(());
                    }
                    Err(meta.error("MetricSet fields support only `name`"))
                })?;
            }
            Ok(quote! {
                if let ::core::option::Option::Some(value) =
                    ::brap::Metric::value(&self.#field_ident)
                {
                    visitor(::brap::metric::Ref {
                        scope: Self::SCOPE,
                        name: #metric_name,
                        value,
                    });
                }
            })
        })
        .collect::<syn::Result<Vec<_>>>()?;

    let ident = &input.ident;
    let generics = &input.generics;
    let (impl_generics, type_generics, where_clause) = generics.split_for_impl();
    Ok(quote! {
        impl #impl_generics ::brap::metric::Set for #ident #type_generics #where_clause {
            const SCOPE: &'static str = #scope;

            fn visit<'a>(
                &'a self,
                visitor: &mut dyn ::core::ops::FnMut(::brap::metric::Ref<'a>),
            ) {
                #(#visits)*
            }
        }
    })
}

/// Measures a synchronous or asynchronous function into an implicitly declared shared atomic
/// histogram. The logarithmic layout is the default. `set = "name"` gates the
/// site on the caller's `brap` and `brap-name` Cargo features.
#[proc_macro_attribute]
pub fn measure(attr: TokenStream, item: TokenStream) -> TokenStream {
    expand_measure(attr, item, None, "measure")
}

/// Measures a synchronous or asynchronous function into an implicitly declared shared atomic
/// mantissa histogram. Precision defaults to four sub-bucket bits. An optional
/// `set` selects an additional compile-time feature gate.
#[proc_macro_attribute]
pub fn measure_fine(attr: TokenStream, item: TokenStream) -> TokenStream {
    expand_measure(attr, item, Some("fine"), "measure_fine")
}

/// Records a lap from the function entry or previous checkpoint. The
/// enclosing measurement attribute consumes this marker and supplies its
/// scope, feature set, histogram, precision, and storage configuration.
#[proc_macro]
pub fn checkpoint(_input: TokenStream) -> TokenStream {
    syn::Error::new(
        Span::call_site(),
        "checkpoint! may only be used inside a function annotated with \
         #[measure] or #[measure_fine]",
    )
    .into_compile_error()
    .into()
}

fn expand_measure(
    attr: TokenStream,
    item: TokenStream,
    forced_histogram: Option<&str>,
    macro_name: &str,
) -> TokenStream {
    let mut args = MeasureArgs::default();
    let parser = syn::meta::parser(|meta| {
        if meta.path.is_ident("name") {
            args.name = Some(meta.value()?.parse()?);
            return Ok(());
        }
        if meta.path.is_ident("scope") {
            args.scope = Some(meta.value()?.parse()?);
            return Ok(());
        }
        if meta.path.is_ident("set") {
            args.set = Some(meta.value()?.parse()?);
            return Ok(());
        }
        if meta.path.is_ident("histogram") {
            args.histogram = Some(meta.value()?.parse()?);
            return Ok(());
        }
        if meta.path.is_ident("precision") {
            args.precision = Some(meta.value()?.parse()?);
            return Ok(());
        }
        if meta.path.is_ident("storage") {
            args.storage = Some(meta.value()?.parse()?);
            return Ok(());
        }
        Err(meta
            .error("supported arguments are name, scope, set, histogram, precision, and storage"))
    });
    parse_macro_input!(attr with parser);
    if let Some(histogram) = forced_histogram {
        if let Some(configured) = args.histogram.as_ref() {
            return syn::Error::new_spanned(
                configured,
                format!("#[{macro_name}] selects its histogram automatically"),
            )
            .into_compile_error()
            .into();
        }
        args.histogram = Some(LitStr::new(histogram, proc_macro2::Span::call_site()));
    }
    if let Some(histogram) = args.histogram.as_ref().filter(|h| h.value() == "count") {
        return syn::Error::new_spanned(
            histogram,
            format!("#[{macro_name}] measures time; use brap::count! to count events"),
        )
        .into_compile_error()
        .into();
    }
    let mut function = parse_macro_input!(item as ItemFn);
    if function.sig.constness.is_some() {
        return syn::Error::new_spanned(
            &function.sig,
            format!("#[{macro_name}] cannot instrument const functions"),
        )
        .into_compile_error()
        .into();
    }
    if function.sig.unsafety.is_some() {
        return syn::Error::new_spanned(
            &function.sig,
            format!("#[{macro_name}] does not yet instrument unsafe functions"),
        )
        .into_compile_error()
        .into();
    }

    let function_name = &function.sig.ident;
    let metric_name = args
        .name
        .as_ref()
        .map(|name| quote!(#name))
        .unwrap_or_else(|| quote!(concat!(module_path!(), "::", stringify!(#function_name))));
    let scope = args
        .scope
        .as_ref()
        .map(|scope| quote!(#scope))
        .unwrap_or_else(|| quote!(module_path!()));
    let storage = match storage_kind(&args) {
        Ok(storage) => storage,
        Err(error) => return error.into_compile_error().into(),
    };
    let (histogram_type, histogram_new) =
        match histogram_tokens(&args, function_name.span(), storage) {
            Ok(histogram) => histogram,
            Err(error) => return error.into_compile_error().into(),
        };
    let site = site_tokens(
        &metric_name,
        &scope,
        &histogram_type,
        &histogram_new,
        storage,
    );
    let record = record_tokens(
        storage,
        quote!(__brap_finished.wrapping_sub(__brap_started)),
    );
    let enabled = match instrumentation_cfg(args.set.as_ref()) {
        Ok(enabled) => enabled,
        Err(error) => return error.into_compile_error().into(),
    };

    let original_block = *function.block;
    let checkpoint_started = Ident::new("__brap_checkpoint_started", Span::mixed_site());
    let mut measured_block = original_block.clone();
    let mut checkpoints = CheckpointRewriter {
        metric_scope: scope.clone(),
        histogram_type: histogram_type.clone(),
        histogram_new: histogram_new.clone(),
        storage,
        checkpoint_started: checkpoint_started.clone(),
        count: 0,
        error: None,
    };
    checkpoints.visit_block_mut(&mut measured_block);
    if let Some(error) = checkpoints.error {
        return error.into_compile_error().into();
    }
    let checkpoint_baseline = (checkpoints.count != 0).then(|| {
        quote! {
            let mut #checkpoint_started = __brap_started;
        }
    });
    let mut unmeasured_block = original_block;
    CheckpointStripper.visit_block_mut(&mut unmeasured_block);
    let measured_call = if function.sig.asyncness.is_some() {
        quote!((async #measured_block).await)
    } else {
        quote!((|| #measured_block)())
    };
    *function.block = syn::parse_quote!({
        #[cfg(#enabled)]
        {
            #site

            let __brap_started = ::brap::now_cycles();
            #checkpoint_baseline
            let __brap_result = #measured_call;
            let __brap_finished = ::brap::now_cycles();
            #record
            __brap_result
        }
        #[cfg(not(#enabled))]
        #unmeasured_block
    });

    quote!(#function).into()
}

struct CheckpointRewriter {
    metric_scope: TokenStream2,
    histogram_type: TokenStream2,
    histogram_new: TokenStream2,
    storage: StorageKind,
    checkpoint_started: Ident,
    count: usize,
    error: Option<syn::Error>,
}

impl CheckpointRewriter {
    fn rewrite(&mut self, invocation: &Macro) -> Option<Expr> {
        if !is_checkpoint(invocation) {
            return None;
        }
        let args = match syn::parse2::<CheckpointArgs>(invocation.tokens.clone()) {
            Ok(args) => args,
            Err(error) => {
                if let Some(errors) = self.error.as_mut() {
                    errors.combine(error);
                } else {
                    self.error = Some(error);
                }
                return Some(syn::parse_quote!(()));
            }
        };

        self.count += 1;
        let name = args.name;
        let scope = &self.metric_scope;
        let histogram_type = &self.histogram_type;
        let histogram_new = &self.histogram_new;
        let storage = self.storage;
        let checkpoint_started = &self.checkpoint_started;
        let site = site_tokens(
            &quote!(#name),
            scope,
            histogram_type,
            histogram_new,
            storage,
        );
        let record = record_tokens(
            storage,
            quote!(__brap_checkpoint_finished.wrapping_sub(#checkpoint_started)),
        );
        Some(syn::parse_quote!({
            #site
            let __brap_checkpoint_finished = ::brap::now_cycles();
            #record
            #checkpoint_started = __brap_checkpoint_finished;
        }))
    }
}

impl VisitMut for CheckpointRewriter {
    fn visit_expr_mut(&mut self, expression: &mut Expr) {
        if let Expr::Macro(expression_macro) = expression
            && let Some(mut replacement) = self.rewrite(&expression_macro.mac)
        {
            set_expr_attrs(&mut replacement, expression_macro.attrs.clone());
            *expression = replacement;
            return;
        }
        visit_mut::visit_expr_mut(self, expression);
    }

    fn visit_stmt_mut(&mut self, statement: &mut Stmt) {
        if let Stmt::Macro(statement_macro) = statement
            && let Some(mut replacement) = self.rewrite(&statement_macro.mac)
        {
            set_expr_attrs(&mut replacement, statement_macro.attrs.clone());
            *statement = Stmt::Expr(replacement, statement_macro.semi_token);
            return;
        }
        visit_mut::visit_stmt_mut(self, statement);
    }

    fn visit_item_mut(&mut self, _item: &mut Item) {
        // A nested item has its own lexical scope. If it is independently
        // measured, its attribute expansion will consume its checkpoints.
    }
}

struct CheckpointStripper;

impl VisitMut for CheckpointStripper {
    fn visit_expr_mut(&mut self, expression: &mut Expr) {
        if let Expr::Macro(expression_macro) = expression
            && is_checkpoint(&expression_macro.mac)
        {
            let mut replacement = syn::parse_quote!(());
            set_expr_attrs(&mut replacement, expression_macro.attrs.clone());
            *expression = replacement;
            return;
        }
        visit_mut::visit_expr_mut(self, expression);
    }

    fn visit_stmt_mut(&mut self, statement: &mut Stmt) {
        if let Stmt::Macro(statement_macro) = statement
            && is_checkpoint(&statement_macro.mac)
        {
            let mut replacement = syn::parse_quote!(());
            set_expr_attrs(&mut replacement, statement_macro.attrs.clone());
            *statement = Stmt::Expr(replacement, statement_macro.semi_token);
            return;
        }
        visit_mut::visit_stmt_mut(self, statement);
    }

    fn visit_item_mut(&mut self, _item: &mut Item) {}
}

fn is_checkpoint(invocation: &Macro) -> bool {
    let path = &invocation.path;
    match path.segments.len() {
        1 => path.is_ident("checkpoint"),
        2 => path.segments[0].ident == "brap" && path.segments[1].ident == "checkpoint",
        _ => false,
    }
}

fn set_expr_attrs(expression: &mut Expr, attrs: Vec<syn::Attribute>) {
    match expression {
        Expr::Block(block) => block.attrs = attrs,
        Expr::Tuple(tuple) => tuple.attrs = attrs,
        _ => unreachable!("generated checkpoint expression supports attributes"),
    }
}

/// Records an already-computed cycle delta into an implicitly declared shared
/// atomic histogram at the invocation site. `storage = "tls"` selects TLS;
/// `set = "name"` selects an additional compile-time feature gate.
#[proc_macro]
pub fn record(input: TokenStream) -> TokenStream {
    let mut args = RecordArgs::default();
    let parser = syn::meta::parser(|meta| {
        if meta.path.is_ident("name") {
            args.site.name = Some(meta.value()?.parse()?);
            return Ok(());
        }
        if meta.path.is_ident("scope") {
            args.site.scope = Some(meta.value()?.parse()?);
            return Ok(());
        }
        if meta.path.is_ident("set") {
            args.site.set = Some(meta.value()?.parse()?);
            return Ok(());
        }
        if meta.path.is_ident("histogram") {
            args.site.histogram = Some(meta.value()?.parse()?);
            return Ok(());
        }
        if meta.path.is_ident("precision") {
            args.site.precision = Some(meta.value()?.parse()?);
            return Ok(());
        }
        if meta.path.is_ident("storage") {
            args.site.storage = Some(meta.value()?.parse()?);
            return Ok(());
        }
        if meta.path.is_ident("cycles") {
            args.cycles = Some(meta.value()?.parse()?);
            return Ok(());
        }
        Err(meta.error(
            "supported arguments are name, scope, set, histogram, precision, storage, and cycles",
        ))
    });
    parse_macro_input!(input with parser);

    let name = match args.site.name.as_ref() {
        Some(name) => quote!(#name),
        None => {
            return syn::Error::new(proc_macro2::Span::call_site(), "record! requires name")
                .into_compile_error()
                .into();
        }
    };
    let scope = args
        .site
        .scope
        .as_ref()
        .map(|scope| quote!(#scope))
        .unwrap_or_else(|| quote!(module_path!()));
    let cycles = match args.cycles {
        Some(cycles) => cycles,
        None => {
            return syn::Error::new(proc_macro2::Span::call_site(), "record! requires cycles")
                .into_compile_error()
                .into();
        }
    };
    let storage = match storage_kind(&args.site) {
        Ok(storage) => storage,
        Err(error) => return error.into_compile_error().into(),
    };
    let (histogram_type, histogram_new) =
        match histogram_tokens(&args.site, proc_macro2::Span::call_site(), storage) {
            Ok(histogram) => histogram,
            Err(error) => return error.into_compile_error().into(),
        };
    let site = site_tokens(&name, &scope, &histogram_type, &histogram_new, storage);
    let record = record_tokens(storage, quote!(__brap_delta_cycles));
    let enabled = match instrumentation_cfg(args.site.set.as_ref()) {
        Ok(enabled) => enabled,
        Err(error) => return error.into_compile_error().into(),
    };

    quote!({
        #[cfg(#enabled)]
        {
            #site
            let __brap_delta_cycles: u64 = #cycles;
            #record
        }
        // The never-called closure keeps bindings used by `#cycles` (typically
        // a `stamp!` result) from warning as unused, without evaluating it.
        #[cfg(not(#enabled))]
        {
            let _ = || #cycles;
        }
    })
    .into()
}

/// Reads the cycle clock only when telemetry for the optional compile set is
/// enabled. Disabled stamps are the constant zero and execute no clock read.
#[proc_macro]
pub fn stamp(input: TokenStream) -> TokenStream {
    let mut set = None;
    let parser = syn::meta::parser(|meta| {
        if meta.path.is_ident("set") {
            set = Some(meta.value()?.parse::<LitStr>()?);
            return Ok(());
        }
        Err(meta.error("the only supported argument is set"))
    });
    parse_macro_input!(input with parser);

    let enabled = match instrumentation_cfg(set.as_ref()) {
        Ok(enabled) => enabled,
        Err(error) => return error.into_compile_error().into(),
    };
    quote!({
        #[cfg(#enabled)]
        {
            ::brap::now_cycles()
        }
        #[cfg(not(#enabled))]
        {
            0_u64
        }
    })
    .into()
}

fn instrumentation_cfg(set: Option<&LitStr>) -> syn::Result<TokenStream2> {
    let Some(set) = set else {
        return Ok(quote!(feature = "brap"));
    };
    let value = set.value();
    let valid = !value.is_empty()
        && value.split('-').all(|part| {
            !part.is_empty()
                && part
                    .bytes()
                    .all(|byte| byte.is_ascii_lowercase() || byte.is_ascii_digit())
        });
    if !valid {
        return Err(syn::Error::new_spanned(
            set,
            "telemetry set names must be nonempty lowercase ASCII words separated by single hyphens",
        ));
    }
    let feature = LitStr::new(&format!("brap-{value}"), set.span());
    Ok(quote!(all(feature = "brap", feature = #feature)))
}

fn histogram_tokens(
    args: &MeasureArgs,
    default_span: proc_macro2::Span,
    storage: StorageKind,
) -> syn::Result<(TokenStream2, TokenStream2)> {
    let histogram = args
        .histogram
        .as_ref()
        .map(LitStr::value)
        .unwrap_or_else(|| "log2".to_owned());
    let precision = match args.precision.as_ref() {
        Some(value) => value.base10_parse::<usize>()?,
        None => 4,
    };

    match histogram.as_str() {
        "log2" => {
            if let Some(precision) = args.precision.as_ref() {
                return Err(syn::Error::new_spanned(
                    precision,
                    "precision applies only to the mantissa histogram",
                ));
            }
            Ok(match storage {
                StorageKind::Atomic => (
                    quote!(::brap::AtomicLog2Histogram),
                    quote!(::brap::AtomicLog2Histogram::new_const()),
                ),
                StorageKind::Tls => (
                    quote!(::brap::Log2Histogram),
                    quote!(::brap::Log2Histogram::new_const()),
                ),
            })
        }
        "mantissa" | "fine" => {
            if !(1..=6).contains(&precision) {
                return Err(syn::Error::new_spanned(
                    args.precision
                        .clone()
                        .unwrap_or_else(|| LitInt::new("4", default_span)),
                    "mantissa precision must be between 1 and 6 bits",
                ));
            }
            let sub_buckets = 1usize << precision;
            let bins = sub_buckets + (64 - precision) * sub_buckets;
            Ok(match storage {
                StorageKind::Atomic => (
                    quote!(::brap::AtomicMantissaHistogram<#bins, #precision>),
                    quote!(::brap::AtomicMantissaHistogram::<#bins, #precision>::new_const()),
                ),
                StorageKind::Tls => (
                    quote!(::brap::MantissaHistogram<#bins, #precision>),
                    quote!(::brap::MantissaHistogram::<#bins, #precision>::new_const()),
                ),
            })
        }
        "count" => {
            if let Some(precision) = args.precision.as_ref() {
                return Err(syn::Error::new_spanned(
                    precision,
                    "precision applies only to the mantissa histogram",
                ));
            }
            Ok(match storage {
                StorageKind::Atomic => (
                    quote!(::brap::AtomicCountHistogram),
                    quote!(::brap::AtomicCountHistogram::new_const()),
                ),
                StorageKind::Tls => (
                    quote!(::brap::CountHistogram),
                    quote!(::brap::CountHistogram::new_const()),
                ),
            })
        }
        _ => Err(syn::Error::new_spanned(
            args.histogram.as_ref().expect("histogram had a value"),
            "histogram must be \"log2\", \"mantissa\", \"fine\", or \"count\"",
        )),
    }
}

fn storage_kind(args: &MeasureArgs) -> syn::Result<StorageKind> {
    match args.storage.as_ref().map(LitStr::value).as_deref() {
        // PERF: shared sites are sharded `BRAP_SHARDS` ways (default 16), so
        // the default matches `tls` up to that many recording threads. Beyond
        // it, threads share shards: with 16 shards, back-to-back `measure`
        // costs ~35 ns at 32 threads and ~82 ns at 60, vs ~20 ns for `tls`.
        None | Some("atomic") => Ok(StorageKind::Atomic),
        Some("tls") => Ok(StorageKind::Tls),
        Some(_) => Err(syn::Error::new_spanned(
            args.storage.as_ref().expect("storage had a value"),
            "storage must be \"atomic\" or \"tls\"",
        )),
    }
}

fn site_tokens(
    metric_name: &TokenStream2,
    scope: &TokenStream2,
    histogram_type: &TokenStream2,
    histogram_new: &TokenStream2,
    storage: StorageKind,
) -> TokenStream2 {
    let (site, layout, storage_token, warm, access) = match storage {
        StorageKind::Atomic => (
            quote! {
                static __BRAP_SITE: ::brap::AtomicSite<#histogram_type> =
                    ::brap::AtomicSite::new(#histogram_new);
            },
            quote!(<#histogram_type as ::brap::AtomicHistogram>::LAYOUT),
            quote!(::brap::MetricStorage::Atomic),
            quote!(__BRAP_SITE.warm()),
            quote! {
                __BRAP_SITE.access(
                    &__BRAP_CONSUMPTION_CALLS,
                    &__BRAP_CONSUMED_SAMPLES,
                )
            },
        ),
        StorageKind::Tls => (
            quote! {
                ::std::thread_local! {
                    static __BRAP_SITE: ::brap::TlsSite<#histogram_type> =
                        const { ::brap::TlsSite::new(#histogram_new) };
                }
            },
            quote!(<#histogram_type as ::brap::Histogram>::LAYOUT),
            quote!(::brap::MetricStorage::Tls),
            quote!(__BRAP_SITE.with(::brap::TlsSite::warm)),
            quote! {
                __BRAP_SITE.with(|site| {
                    site.access(
                        &__BRAP_CONSUMPTION_CALLS,
                        &__BRAP_CONSUMED_SAMPLES,
                    )
                })
            },
        ),
    };

    quote! {
        #site
        static __BRAP_FRESH_ACCESSES: ::core::sync::atomic::AtomicU64 =
            ::core::sync::atomic::AtomicU64::new(0);
        static __BRAP_CONSUMPTION_CALLS: ::core::sync::atomic::AtomicU64 =
            ::core::sync::atomic::AtomicU64::new(0);
        static __BRAP_CONSUMED_SAMPLES: ::core::sync::atomic::AtomicU64 =
            ::core::sync::atomic::AtomicU64::new(0);

        #[::brap::__private::distributed_slice(::brap::METRICS)]
        #[linkme(crate = ::brap::__private)]
        static __BRAP_DESCRIPTOR: ::brap::metric::Descriptor = {
            fn warm() {
                #warm;
            }
            fn access() -> ::core::option::Option<::brap::histogram::Snapshot> {
                #access
            }
            ::brap::metric::Descriptor::new(
                #metric_name,
                #scope,
                #layout,
                #storage_token,
                warm,
                access,
                &__BRAP_FRESH_ACCESSES,
                &__BRAP_CONSUMPTION_CALLS,
                &__BRAP_CONSUMED_SAMPLES,
            )
        };
    }
}

fn record_tokens(storage: StorageKind, delta_cycles: TokenStream2) -> TokenStream2 {
    match storage {
        StorageKind::Atomic => quote! {
            __BRAP_SITE.record(#delta_cycles);
        },
        StorageKind::Tls => quote! {
            __BRAP_SITE.with(|site| {
                site.record(#delta_cycles, &__BRAP_FRESH_ACCESSES)
            });
        },
    }
}
