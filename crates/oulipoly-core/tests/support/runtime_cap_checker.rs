use quote::ToTokens;
use std::collections::{BTreeMap, BTreeSet};
use std::fs;
use std::path::{Path, PathBuf};
use syn::visit::Visit;

#[derive(Debug, Clone, PartialEq, Eq, PartialOrd, Ord)]
pub struct Site {
    pub source: String,
    pub scope: String,
    pub symbol: String,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Declaration {
    pub site: Site,
    pub value_expression: String,
}

#[derive(Debug, Clone, PartialEq, Eq, PartialOrd, Ord)]
pub struct UseSite {
    pub source: String,
    pub scope: String,
    pub symbol: String,
}

#[derive(Debug, Default)]
pub struct RustInventory {
    pub declarations: BTreeMap<Site, Declaration>,
    pub uses: BTreeSet<UseSite>,
    pub raw_cap_literals: Vec<String>,
}

pub fn rust_sources(root: &Path) -> Vec<PathBuf> {
    let mut files = Vec::new();
    for source_root in [root.join("crates"), root.join("src-tauri/src")] {
        collect_rust_sources(&source_root, &mut files);
    }
    files.sort();
    let test_only = mechanically_test_only_sources(&files);
    files
        .into_iter()
        .filter(|path| !cargo_integration_test_source(path) && !test_only.contains(path))
        .collect()
}

fn collect_rust_sources(path: &Path, files: &mut Vec<PathBuf>) {
    let Ok(entries) = fs::read_dir(path) else {
        return;
    };
    for entry in entries.flatten() {
        let path = entry.path();
        if path.is_dir() {
            collect_rust_sources(&path, files);
        } else if path.extension().and_then(|extension| extension.to_str()) == Some("rs") {
            files.push(path);
        }
    }
}

fn cargo_integration_test_source(path: &Path) -> bool {
    let components = path
        .components()
        .map(|component| component.as_os_str().to_string_lossy())
        .collect::<Vec<_>>();
    components
        .windows(3)
        .any(|parts| parts[0] == "crates" && parts[2] == "tests")
}

#[derive(Debug)]
struct ModuleOwnership {
    owner: PathBuf,
    target: PathBuf,
    declaration_is_test_only: bool,
}

pub fn mechanically_test_only_sources(files: &[PathBuf]) -> BTreeSet<PathBuf> {
    let available = files.iter().cloned().collect::<BTreeSet<_>>();
    let mut file_test_only = BTreeSet::new();
    let mut ownership = Vec::new();
    for owner in files {
        let Ok(source) = fs::read_to_string(owner) else {
            continue;
        };
        let Ok(file) = syn::parse_file(&source) else {
            continue;
        };
        if cfg_test(&file.attrs) {
            file_test_only.insert(owner.clone());
        }
        collect_module_ownership(
            owner,
            &file.items,
            false,
            module_base(owner),
            &available,
            &mut ownership,
        );
    }
    let owned = ownership
        .iter()
        .map(|edge| edge.target.clone())
        .collect::<BTreeSet<_>>();
    let mut production = available
        .iter()
        .filter(|path| !owned.contains(*path) && !file_test_only.contains(*path))
        .cloned()
        .collect::<BTreeSet<_>>();
    loop {
        let before = production.len();
        for edge in &ownership {
            if production.contains(&edge.owner)
                && !edge.declaration_is_test_only
                && !file_test_only.contains(&edge.target)
            {
                production.insert(edge.target.clone());
            }
        }
        if production.len() == before {
            break;
        }
    }
    available.difference(&production).cloned().collect()
}

fn module_base(owner: &Path) -> PathBuf {
    let parent = owner.parent().unwrap_or_else(|| Path::new(""));
    match owner.file_stem().and_then(|stem| stem.to_str()) {
        Some("lib" | "main" | "mod") | None => parent.to_path_buf(),
        Some(stem) => parent.join(stem),
    }
}

fn collect_module_ownership(
    owner: &Path,
    items: &[syn::Item],
    inherited_test_only: bool,
    base: PathBuf,
    available: &BTreeSet<PathBuf>,
    ownership: &mut Vec<ModuleOwnership>,
) {
    for item in items {
        let syn::Item::Mod(module) = item else {
            continue;
        };
        let module_test_only = inherited_test_only || cfg_test(&module.attrs);
        if let Some((_, nested)) = &module.content {
            collect_module_ownership(
                owner,
                nested,
                module_test_only,
                base.join(module.ident.to_string()),
                available,
                ownership,
            );
            continue;
        }
        let explicit_path =
            module.attrs.iter().find_map(|attribute| {
                attribute
                    .path()
                    .is_ident("path")
                    .then(|| {
                        attribute.meta.require_name_value().ok().and_then(|value| {
                            match &value.value {
                                syn::Expr::Lit(syn::ExprLit {
                                    lit: syn::Lit::Str(path),
                                    ..
                                }) => Some(path.value()),
                                _ => None,
                            }
                        })
                    })
                    .flatten()
            });
        let candidates = if let Some(explicit_path) = explicit_path {
            vec![
                owner
                    .parent()
                    .unwrap_or_else(|| Path::new(""))
                    .join(explicit_path),
            ]
        } else {
            let name = module.ident.to_string();
            vec![
                base.join(format!("{name}.rs")),
                base.join(name).join("mod.rs"),
            ]
        };
        for target in candidates {
            if available.contains(&target) {
                ownership.push(ModuleOwnership {
                    owner: owner.to_path_buf(),
                    target,
                    declaration_is_test_only: module_test_only,
                });
            }
        }
    }
}

pub fn scan_workspace_rust(root: &Path) -> RustInventory {
    let mut inventory = RustInventory::default();
    for path in rust_sources(root) {
        let relative = path
            .strip_prefix(root)
            .unwrap()
            .to_string_lossy()
            .replace('\\', "/");
        let source = fs::read_to_string(&path).unwrap();
        let scanned = scan_rust_source(&relative, &source).unwrap_or_else(|error| {
            panic!("failed to parse {relative} while checking runtime caps: {error}")
        });
        for (site, declaration) in scanned.declarations {
            assert!(
                inventory
                    .declarations
                    .insert(site.clone(), declaration)
                    .is_none(),
                "duplicate discovered declaration site: {site:?}"
            );
        }
        inventory.uses.extend(scanned.uses);
        inventory.raw_cap_literals.extend(scanned.raw_cap_literals);
    }
    inventory
}

pub fn scan_rust_source(relative: &str, source: &str) -> Result<RustInventory, syn::Error> {
    let syntax = syn::parse_file(source)?;
    let mut visitor = CapVisitor::new(relative);
    visitor.visit_file(&syntax);
    Ok(visitor.inventory)
}

struct CapVisitor {
    source: String,
    scopes: Vec<String>,
    inventory: RustInventory,
}

impl CapVisitor {
    fn new(source: &str) -> Self {
        Self {
            source: source.to_string(),
            scopes: vec!["module".to_string()],
            inventory: RustInventory::default(),
        }
    }

    fn scope(&self) -> String {
        self.scopes.join("::")
    }

    fn with_scope(&mut self, scope: String, visit: impl FnOnce(&mut Self)) {
        self.scopes.push(scope);
        visit(self);
        self.scopes.pop();
    }

    fn record_declaration(&mut self, symbol: &syn::Ident, ty: &syn::Type, value: &syn::Expr) {
        if !numeric_type(ty) && !type_ends_with_duration(ty) {
            return;
        }
        let site = Site {
            source: self.source.clone(),
            scope: self.scope(),
            symbol: symbol.to_string(),
        };
        let declaration = Declaration {
            site: site.clone(),
            value_expression: normalized_tokens(value),
        };
        assert!(
            self.inventory
                .declarations
                .insert(site.clone(), declaration)
                .is_none(),
            "duplicate declaration at {site:?}"
        );
    }

    fn record_use(&mut self, symbol: &syn::Ident) {
        let symbol = symbol.to_string();
        if !constant_style_identifier(&symbol) {
            return;
        }
        self.inventory.uses.insert(UseSite {
            source: self.source.clone(),
            scope: self.scope(),
            symbol,
        });
    }

    fn raw(&mut self, detail: impl Into<String>) {
        self.inventory.raw_cap_literals.push(format!(
            "{}#{}: {}",
            self.source,
            self.scope(),
            detail.into()
        ));
    }

    fn checks_anonymous_literals(&self) -> bool {
        self.scopes
            .last()
            .is_some_and(|scope| scope.starts_with("fn:"))
    }

    fn with_cfg_scope(&mut self, attrs: &[syn::Attribute], visit: impl FnOnce(&mut Self)) {
        if let Some(scope) = cfg_scope(attrs) {
            self.with_scope(scope, visit);
        } else {
            visit(self);
        }
    }
}

impl<'ast> Visit<'ast> for CapVisitor {
    fn visit_item(&mut self, item: &'ast syn::Item) {
        if item_attrs(item).is_some_and(cfg_test) {
            return;
        }
        syn::visit::visit_item(self, item);
    }

    fn visit_item_mod(&mut self, item: &'ast syn::ItemMod) {
        if cfg_test(&item.attrs) {
            return;
        }
        if let Some((_, items)) = &item.content {
            self.with_scope(format!("mod:{}", item.ident), |visitor| {
                for item in items {
                    visitor.visit_item(item);
                }
            });
        }
    }

    fn visit_item_impl(&mut self, item: &'ast syn::ItemImpl) {
        if cfg_test(&item.attrs) {
            return;
        }
        let ty = normalized_tokens(item.self_ty.as_ref());
        self.with_scope(format!("impl:{ty}"), |visitor| {
            for item in &item.items {
                visitor.visit_impl_item(item);
            }
        });
    }

    fn visit_item_const(&mut self, item: &'ast syn::ItemConst) {
        if cfg_test(&item.attrs) {
            return;
        }
        self.with_cfg_scope(&item.attrs, |visitor| {
            visitor.record_declaration(&item.ident, &item.ty, &item.expr);
            visitor.with_scope(format!("const:{}", item.ident), |visitor| {
                visitor.visit_expr(&item.expr)
            });
        });
    }

    fn visit_item_static(&mut self, item: &'ast syn::ItemStatic) {
        if cfg_test(&item.attrs) {
            return;
        }
        self.with_cfg_scope(&item.attrs, |visitor| {
            visitor.record_declaration(&item.ident, &item.ty, &item.expr);
            visitor.with_scope(format!("static:{}", item.ident), |visitor| {
                visitor.visit_expr(&item.expr)
            });
        });
    }

    fn visit_item_fn(&mut self, item: &'ast syn::ItemFn) {
        if cfg_test(&item.attrs) || has_test_attr(&item.attrs) {
            return;
        }
        self.with_scope(format!("fn:{}", item.sig.ident), |visitor| {
            visitor.visit_block(&item.block)
        });
    }

    fn visit_impl_item_const(&mut self, item: &'ast syn::ImplItemConst) {
        if cfg_test(&item.attrs) {
            return;
        }
        self.with_cfg_scope(&item.attrs, |visitor| {
            visitor.record_declaration(&item.ident, &item.ty, &item.expr);
            visitor.with_scope(format!("const:{}", item.ident), |visitor| {
                visitor.visit_expr(&item.expr)
            });
        });
    }

    fn visit_impl_item_fn(&mut self, item: &'ast syn::ImplItemFn) {
        if cfg_test(&item.attrs) {
            return;
        }
        self.with_scope(format!("fn:{}", item.sig.ident), |visitor| {
            visitor.visit_block(&item.block)
        });
    }

    fn visit_trait_item_const(&mut self, item: &'ast syn::TraitItemConst) {
        if cfg_test(&item.attrs) {
            return;
        }
        if let Some((_, value)) = &item.default {
            self.record_declaration(&item.ident, &item.ty, value);
            self.with_scope(format!("const:{}", item.ident), |visitor| {
                visitor.visit_expr(value)
            });
        }
    }

    fn visit_expr_path(&mut self, expression: &'ast syn::ExprPath) {
        if let Some(segment) = expression.path.segments.last() {
            self.record_use(&segment.ident);
        }
        syn::visit::visit_expr_path(self, expression);
    }

    fn visit_expr_macro(&mut self, expression: &'ast syn::ExprMacro) {
        visit_token_identifiers(expression.mac.tokens.clone(), &mut |ident| {
            self.record_use(&ident)
        });
        syn::visit::visit_expr_macro(self, expression);
    }

    fn visit_expr_call(&mut self, call: &'ast syn::ExprCall) {
        if self.checks_anonymous_literals() && literal_duration_constructor(call) {
            self.raw("direct literal duration constructor");
        }
        if self.checks_anonymous_literals()
            && let syn::Expr::Path(function) = call.func.as_ref()
        {
            let path = function
                .path
                .segments
                .iter()
                .map(|segment| segment.ident.to_string())
                .collect::<Vec<_>>();
            let last = path.last().map(String::as_str);
            if matches!(last, Some("sync_channel" | "bounded"))
                && call.args.iter().any(positive_literal_integer)
            {
                self.raw(format!("literal capacity passed to {}", path.join("::")));
            }
            if path.ends_with(&["libc".into(), "poll".into()])
                && call
                    .args
                    .iter()
                    .nth(2)
                    .is_some_and(positive_literal_integer)
            {
                self.raw("literal timeout passed to libc::poll");
            }
            if matches!(last, Some("usleep" | "sleep"))
                && path.iter().any(|segment| segment == "libc")
                && call.args.iter().any(positive_literal_integer)
            {
                self.raw(format!("literal interval passed to {}", path.join("::")));
            }
        }
        syn::visit::visit_expr_call(self, call);
    }

    fn visit_expr_method_call(&mut self, call: &'ast syn::ExprMethodCall) {
        if self.checks_anonymous_literals()
            && matches!(
                call.method.to_string().as_str(),
                "read_to_end" | "read_to_string"
            )
            && let syn::Expr::MethodCall(bound) = call.receiver.as_ref()
            && bound.method == "take"
            && bound.args.iter().any(positive_literal_integer)
        {
            self.raw("literal I/O bound passed to Read::take");
        }
        syn::visit::visit_expr_method_call(self, call);
    }

    fn visit_expr_struct(&mut self, expression: &'ast syn::ExprStruct) {
        let type_name = expression
            .path
            .segments
            .last()
            .map(|segment| segment.ident.to_string());
        let cap_configuration = self.scope().ends_with("::fn:default")
            || type_name.as_deref().is_some_and(|name| {
                ["Config", "Limits", "Options", "Request"]
                    .iter()
                    .any(|suffix| name.ends_with(suffix))
            });
        if self.checks_anonymous_literals() && cap_configuration {
            for field in &expression.fields {
                let member = match &field.member {
                    syn::Member::Named(member) => member.to_string(),
                    syn::Member::Unnamed(_) => continue,
                };
                if cap_field_name(&member) && literal_numeric_expression(&field.expr) {
                    self.raw(format!(
                        "literal cap value assigned to {}.{}",
                        normalized_tokens(&expression.path),
                        member
                    ));
                }
            }
        }
        syn::visit::visit_expr_struct(self, expression);
    }

    fn visit_expr_repeat(&mut self, expression: &'ast syn::ExprRepeat) {
        if self.checks_anonymous_literals()
            && literal_numeric_value(&expression.len).is_some_and(|length| length >= 1024)
        {
            self.raw("literal fixed-size allocation of at least 1 KiB");
        }
        syn::visit::visit_expr_repeat(self, expression);
    }
}

fn visit_token_identifiers(stream: proc_macro2::TokenStream, visit: &mut impl FnMut(syn::Ident)) {
    for token in stream {
        match token {
            proc_macro2::TokenTree::Ident(ident) => visit(ident),
            proc_macro2::TokenTree::Group(group) => visit_token_identifiers(group.stream(), visit),
            _ => {}
        }
    }
}

fn normalized_tokens(value: &impl ToTokens) -> String {
    value.to_token_stream().to_string()
}

fn constant_style_identifier(value: &str) -> bool {
    !value.is_empty()
        && value
            .bytes()
            .all(|byte| byte == b'_' || byte.is_ascii_uppercase() || byte.is_ascii_digit())
        && value.bytes().any(|byte| byte.is_ascii_uppercase())
}

fn positive_literal_integer(expression: &syn::Expr) -> bool {
    literal_numeric_value(expression).is_some_and(|value| value > 0)
}

fn literal_numeric_expression(expression: &syn::Expr) -> bool {
    match expression {
        syn::Expr::Lit(syn::ExprLit {
            lit: syn::Lit::Int(_) | syn::Lit::Float(_),
            ..
        }) => true,
        syn::Expr::Binary(binary) => {
            literal_numeric_expression(&binary.left) && literal_numeric_expression(&binary.right)
        }
        syn::Expr::Cast(cast) => literal_numeric_expression(&cast.expr),
        syn::Expr::Group(group) => literal_numeric_expression(&group.expr),
        syn::Expr::Paren(paren) => literal_numeric_expression(&paren.expr),
        syn::Expr::Unary(unary) => literal_numeric_expression(&unary.expr),
        _ => false,
    }
}

fn literal_numeric_value(expression: &syn::Expr) -> Option<u128> {
    match expression {
        syn::Expr::Lit(syn::ExprLit {
            lit: syn::Lit::Int(value),
            ..
        }) => value.base10_parse().ok(),
        syn::Expr::Binary(binary) => {
            let left = literal_numeric_value(&binary.left)?;
            let right = literal_numeric_value(&binary.right)?;
            match binary.op {
                syn::BinOp::Add(_) => left.checked_add(right),
                syn::BinOp::Sub(_) => left.checked_sub(right),
                syn::BinOp::Mul(_) => left.checked_mul(right),
                syn::BinOp::Div(_) if right != 0 => left.checked_div(right),
                _ => None,
            }
        }
        syn::Expr::Cast(cast) => literal_numeric_value(&cast.expr),
        syn::Expr::Group(group) => literal_numeric_value(&group.expr),
        syn::Expr::Paren(paren) => literal_numeric_value(&paren.expr),
        _ => None,
    }
}

fn cap_field_name(name: &str) -> bool {
    let name = name.to_ascii_lowercase();
    name.contains("timeout")
        || name.contains("deadline")
        || name.contains("interval")
        || name.contains("retry")
        || name.contains("attempt")
        || name.contains("capacity")
        || name.contains("limit")
        || name.contains("max_")
        || name.contains("_max")
        || name.contains("ttl")
        || name.contains("stale_after")
        || name.ends_with("_bytes")
        || name.ends_with("_depth")
        || name.ends_with("_cap")
}

fn literal_duration_constructor(call: &syn::ExprCall) -> bool {
    let syn::Expr::Path(function) = call.func.as_ref() else {
        return false;
    };
    let mut segments = function.path.segments.iter().rev();
    let Some(constructor) = segments.next() else {
        return false;
    };
    let Some(duration) = segments.next() else {
        return false;
    };
    let duration_type = duration.ident.to_string();
    let constructor = constructor.ident.to_string();
    matches!(
        duration_type.as_str(),
        "Duration" | "StdDuration" | "ChronoDuration" | "TimeDelta"
    ) && (constructor == "new"
        || constructor.starts_with("from_")
        || matches!(
            constructor.as_str(),
            "nanoseconds"
                | "microseconds"
                | "milliseconds"
                | "seconds"
                | "minutes"
                | "hours"
                | "days"
                | "weeks"
        ))
        && call.args.iter().any(literal_numeric_expression)
}

fn item_attrs(item: &syn::Item) -> Option<&[syn::Attribute]> {
    Some(match item {
        syn::Item::Const(item) => &item.attrs,
        syn::Item::Enum(item) => &item.attrs,
        syn::Item::ExternCrate(item) => &item.attrs,
        syn::Item::Fn(item) => &item.attrs,
        syn::Item::ForeignMod(item) => &item.attrs,
        syn::Item::Impl(item) => &item.attrs,
        syn::Item::Macro(item) => &item.attrs,
        syn::Item::Mod(item) => &item.attrs,
        syn::Item::Static(item) => &item.attrs,
        syn::Item::Struct(item) => &item.attrs,
        syn::Item::Trait(item) => &item.attrs,
        syn::Item::TraitAlias(item) => &item.attrs,
        syn::Item::Type(item) => &item.attrs,
        syn::Item::Union(item) => &item.attrs,
        syn::Item::Use(item) => &item.attrs,
        _ => return None,
    })
}

fn cfg_test(attrs: &[syn::Attribute]) -> bool {
    attrs.iter().any(|attribute| {
        if !attribute.path().is_ident("cfg") {
            return false;
        }
        attribute.meta.require_list().is_ok_and(|list| {
            let tokens = list.tokens.to_string().replace(' ', "");
            !tokens.starts_with("not(")
                && (tokens.contains("test") || tokens.contains("feature=\"test-support\""))
        })
    })
}

fn cfg_scope(attrs: &[syn::Attribute]) -> Option<String> {
    attrs.iter().find_map(|attribute| {
        if !attribute.path().is_ident("cfg") {
            return None;
        }
        attribute
            .meta
            .require_list()
            .ok()
            .map(|list| format!("cfg:{}", list.tokens.to_string().replace(' ', "")))
    })
}

fn has_test_attr(attrs: &[syn::Attribute]) -> bool {
    attrs
        .iter()
        .any(|attribute| attribute.path().is_ident("test"))
}

fn numeric_type(ty: &syn::Type) -> bool {
    match ty {
        syn::Type::Array(array) => numeric_type(&array.elem),
        syn::Type::Group(group) => numeric_type(&group.elem),
        syn::Type::Paren(paren) => numeric_type(&paren.elem),
        syn::Type::Reference(reference) => numeric_type(&reference.elem),
        syn::Type::Slice(slice) => numeric_type(&slice.elem),
        syn::Type::Tuple(tuple) => tuple.elems.iter().any(numeric_type),
        syn::Type::Path(path) => path.path.segments.last().is_some_and(|segment| {
            matches!(
                segment.ident.to_string().as_str(),
                "usize"
                    | "u128"
                    | "u64"
                    | "u32"
                    | "u16"
                    | "u8"
                    | "isize"
                    | "i128"
                    | "i64"
                    | "i32"
                    | "i16"
                    | "i8"
                    | "f64"
                    | "f32"
            ) || match &segment.arguments {
                syn::PathArguments::AngleBracketed(arguments) => arguments.args.iter().any(|arg| {
                    matches!(arg, syn::GenericArgument::Type(ty) if numeric_type(ty) || type_ends_with_duration(ty))
                }),
                _ => false,
            }
        }),
        _ => false,
    }
}

fn type_ends_with_duration(ty: &syn::Type) -> bool {
    let syn::Type::Path(path) = ty else {
        return false;
    };
    path.path.segments.last().is_some_and(|segment| {
        matches!(
            segment.ident.to_string().as_str(),
            "Duration" | "StdDuration" | "ChronoDuration" | "TimeDelta"
        )
    })
}

#[derive(Debug, Clone, PartialEq, Eq, PartialOrd, Ord)]
pub struct ScriptDeclaration {
    pub source: String,
    pub symbol: String,
    pub value_expression: String,
    pub used: bool,
}

#[derive(Debug, Default)]
pub struct ScriptInventory {
    pub declarations: BTreeSet<ScriptDeclaration>,
    pub raw_cap_literals: Vec<String>,
}

pub fn shipped_scripts(root: &Path) -> Vec<PathBuf> {
    let scripts = root.join("scripts");
    let mut paths = fs::read_dir(scripts)
        .unwrap()
        .flatten()
        .map(|entry| entry.path())
        .filter(|path| path.is_file())
        .filter(|path| path.file_name().and_then(|name| name.to_str()) != Some("README.md"))
        .filter(|path| {
            path.extension().and_then(|extension| extension.to_str()) == Some("sh")
                || path.extension().is_none()
                || executable(path)
        })
        .collect::<Vec<_>>();
    paths.sort();
    paths
}

#[cfg(unix)]
fn executable(path: &Path) -> bool {
    use std::os::unix::fs::PermissionsExt;
    fs::metadata(path).is_ok_and(|metadata| metadata.permissions().mode() & 0o111 != 0)
}

#[cfg(not(unix))]
fn executable(_path: &Path) -> bool {
    false
}

pub fn scan_shipped_scripts(root: &Path) -> ScriptInventory {
    let mut inventory = ScriptInventory::default();
    for path in shipped_scripts(root) {
        let relative = path
            .strip_prefix(root)
            .unwrap()
            .to_string_lossy()
            .replace('\\', "/");
        let source = fs::read_to_string(&path).unwrap();
        let scanned = scan_script_source(&relative, &source);
        inventory.declarations.extend(scanned.declarations);
        inventory.raw_cap_literals.extend(scanned.raw_cap_literals);
    }
    inventory
}

pub fn scan_script_source(relative: &str, source: &str) -> ScriptInventory {
    let mut inventory = ScriptInventory::default();
    let lines = source.lines().collect::<Vec<_>>();
    for (index, line) in lines.iter().enumerate() {
        let trimmed = line.trim();
        if trimmed.starts_with('#') {
            continue;
        }
        if let Some((left, right)) = trimmed.split_once('=') {
            let symbol = left.trim_start_matches("export ").trim();
            if constant_style_identifier(symbol) && numeric_shell_expression(right.trim()) {
                let used = lines.iter().enumerate().any(|(other, line)| {
                    other != index
                        && (line.contains(&format!("${symbol}"))
                            || line.contains(&format!("${{{symbol}"))
                            || contains_identifier(line, symbol))
                });
                inventory.declarations.insert(ScriptDeclaration {
                    source: relative.to_string(),
                    symbol: symbol.to_string(),
                    value_expression: right.trim().to_string(),
                    used,
                });
            }
        }
        let words = shell_words(trimmed);
        for pair in words.windows(2) {
            if matches!(
                pair[0].as_str(),
                "--max-time" | "--connect-timeout" | "--timeout" | "timeout" | "sleep"
            ) && pair[1].parse::<f64>().is_ok()
            {
                inventory.raw_cap_literals.push(format!(
                    "{relative}:{}: literal {} passed to {}",
                    index + 1,
                    pair[1],
                    pair[0]
                ));
            }
        }
        for word in &words {
            if let Some((option, value)) = word.split_once('=')
                && matches!(option, "--max-time" | "--connect-timeout" | "--timeout")
                && value.parse::<f64>().is_ok()
            {
                inventory.raw_cap_literals.push(format!(
                    "{relative}:{}: literal {value} passed to {option}",
                    index + 1
                ));
            }
        }
        if let Some(value) = numeric_keyword_argument(trimmed, "timeout=") {
            inventory.raw_cap_literals.push(format!(
                "{relative}:{}: literal {value} passed to timeout=",
                index + 1
            ));
        }
    }
    inventory
}

fn numeric_keyword_argument<'a>(line: &'a str, keyword: &str) -> Option<&'a str> {
    let (start, _) = line.match_indices(keyword).find(|(start, _)| {
        !line[..*start].chars().next_back().is_some_and(|character| {
            character.is_ascii_alphanumeric() || matches!(character, '_' | '-')
        })
    })?;
    let tail = &line[start + keyword.len()..];
    let value = tail
        .trim_start()
        .split(|character: char| {
            character.is_whitespace() || matches!(character, ',' | ')' | ']' | '}')
        })
        .next()?
        .trim_matches(['\'', '"']);
    value.parse::<f64>().is_ok().then_some(value)
}

fn contains_identifier(line: &str, symbol: &str) -> bool {
    line.match_indices(symbol).any(|(start, _)| {
        let before = line[..start].chars().next_back();
        let end = start + symbol.len();
        let after = line[end..].chars().next();
        !before.is_some_and(|character| character.is_ascii_alphanumeric() || character == '_')
            && !after.is_some_and(|character| character.is_ascii_alphanumeric() || character == '_')
    })
}

fn numeric_shell_expression(value: &str) -> bool {
    let unquoted = value.trim_matches(['\'', '"']);
    unquoted
        .chars()
        .next()
        .is_some_and(|character| character.is_ascii_digit())
        || unquoted.contains(":-")
            && unquoted
                .split(":-")
                .nth(1)
                .and_then(|tail| tail.chars().find(|character| !character.is_whitespace()))
                .is_some_and(|character| character.is_ascii_digit())
}

fn shell_words(line: &str) -> Vec<String> {
    line.split_whitespace()
        .map(|word| word.trim_matches(['\'', '"', ';', '\\']).to_string())
        .collect()
}
