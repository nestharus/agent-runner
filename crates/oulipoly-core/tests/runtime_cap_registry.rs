use oulipoly_core::runtime_cap::{RuntimeCap, RuntimeCapClass, registry, validate_registry};
use std::collections::{BTreeMap, BTreeSet};
use std::fs;
use std::path::{Path, PathBuf};
use syn::visit::Visit;

fn workspace() -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR"))
        .parent()
        .and_then(Path::parent)
        .expect("core crate must be below workspace/crates")
        .to_path_buf()
}

fn rust_sources(root: &Path) -> Vec<PathBuf> {
    let mut files = Vec::new();
    for source_root in [root.join("crates"), root.join("src-tauri/src")] {
        collect_rust_sources(&source_root, &mut files);
    }
    files.sort();
    files
}

fn collect_rust_sources(path: &Path, files: &mut Vec<PathBuf>) {
    let Ok(entries) = fs::read_dir(path) else {
        return;
    };
    for entry in entries.flatten() {
        let path = entry.path();
        if path.is_dir() {
            if path.file_name().and_then(|name| name.to_str()) != Some("tests") {
                collect_rust_sources(&path, files);
            }
        } else if path.extension().and_then(|extension| extension.to_str()) == Some("rs")
            && !path
                .file_name()
                .and_then(|name| name.to_str())
                .is_some_and(|name| name.ends_with("_tests.rs") || name.ends_with("_test.rs"))
            && !mechanically_test_only_source(&path)
        {
            files.push(path);
        }
    }
}

fn mechanically_test_only_source(path: &Path) -> bool {
    let Some(name) = path.file_name().and_then(|name| name.to_str()) else {
        return false;
    };
    if name == "testkit.rs" {
        let parent = path.parent().unwrap().join("lib.rs");
        return fs::read_to_string(parent)
            .is_ok_and(|source| source.contains("#[cfg(test)]\nmod testkit;"));
    }
    let Some(directory) = path.parent() else {
        return false;
    };
    fs::read_dir(directory).is_ok_and(|entries| {
        entries.flatten().any(|entry| {
            let owner = entry.path();
            owner
                .file_name()
                .and_then(|file| file.to_str())
                .is_some_and(|file| file.ends_with("_tests.rs") || file.ends_with("_test.rs"))
                && fs::read_to_string(owner)
                    .is_ok_and(|source| source.contains(&format!("#[path = \"{name}\"]")))
        })
    })
}

#[derive(Default)]
struct CapVisitor {
    symbols: BTreeSet<String>,
    raw_terminal_durations: Vec<String>,
    raw_duration_literals: Vec<String>,
    raw_resource_bounds: Vec<String>,
    current_function: Option<String>,
}

impl<'ast> Visit<'ast> for CapVisitor {
    fn visit_item(&mut self, item: &'ast syn::Item) {
        if item_attrs(item).is_some_and(cfg_test) {
            return;
        }
        match item {
            syn::Item::Const(item) if !cfg_test(&item.attrs) => {
                if cap_shaped(&item.ident.to_string(), &item.ty) {
                    self.symbols.insert(item.ident.to_string());
                }
            }
            syn::Item::Static(item) if !cfg_test(&item.attrs) => {
                if cap_shaped(&item.ident.to_string(), &item.ty) {
                    self.symbols.insert(item.ident.to_string());
                }
            }
            syn::Item::Mod(item) if cfg_test(&item.attrs) => {}
            syn::Item::Fn(item) if cfg_test(&item.attrs) || has_test_attr(&item.attrs) => {}
            _ => syn::visit::visit_item(self, item),
        }
    }

    fn visit_expr_method_call(&mut self, call: &'ast syn::ExprMethodCall) {
        const TERMINAL_APIS: &[&str] = &[
            "busy_timeout",
            "recv_timeout",
            "set_read_timeout",
            "set_write_timeout",
            "wait_timeout",
            "with_timeout",
        ];
        if TERMINAL_APIS.contains(&call.method.to_string().as_str())
            && call.args.iter().any(contains_literal_duration)
        {
            self.raw_terminal_durations.push(call.method.to_string());
        }
        if matches!(call.method.to_string().as_str(), "take" | "truncate")
            && call.args.iter().any(literal_integer)
        {
            self.raw_resource_bounds.push(format!(
                "literal passed to {} in {}",
                call.method,
                self.current_function
                    .as_deref()
                    .unwrap_or("<non-function expression>")
            ));
        }
        syn::visit::visit_expr_method_call(self, call);
    }

    fn visit_item_fn(&mut self, item: &'ast syn::ItemFn) {
        if cfg_test(&item.attrs) || has_test_attr(&item.attrs) {
            return;
        }
        let previous = self.current_function.replace(item.sig.ident.to_string());
        syn::visit::visit_item_fn(self, item);
        self.current_function = previous;
    }

    fn visit_impl_item(&mut self, item: &'ast syn::ImplItem) {
        match item {
            syn::ImplItem::Const(item) if !cfg_test(&item.attrs) => {
                if cap_shaped(&item.ident.to_string(), &item.ty) {
                    self.symbols.insert(item.ident.to_string());
                }
            }
            syn::ImplItem::Fn(item) if !cfg_test(&item.attrs) => {
                let previous = self.current_function.replace(item.sig.ident.to_string());
                syn::visit::visit_impl_item_fn(self, item);
                self.current_function = previous;
            }
            syn::ImplItem::Fn(_) | syn::ImplItem::Const(_) => {}
            _ => syn::visit::visit_impl_item(self, item),
        }
    }

    fn visit_expr_call(&mut self, call: &'ast syn::ExprCall) {
        if literal_duration_constructor(call) {
            self.raw_duration_literals.push(
                self.current_function
                    .clone()
                    .unwrap_or_else(|| "<non-function expression>".into()),
            );
        }
        if let syn::Expr::Path(function) = call.func.as_ref()
            && function
                .path
                .segments
                .last()
                .is_some_and(|segment| segment.ident == "sync_channel")
            && call.args.iter().any(literal_integer)
        {
            self.raw_resource_bounds.push(format!(
                "literal sync_channel capacity in {}",
                self.current_function
                    .as_deref()
                    .unwrap_or("<non-function expression>")
            ));
        }
        syn::visit::visit_expr_call(self, call);
    }
}

fn literal_integer(expression: &syn::Expr) -> bool {
    matches!(
        expression,
        syn::Expr::Lit(syn::ExprLit {
            lit: syn::Lit::Int(_),
            ..
        })
    )
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
    matches!(
        duration.ident.to_string().as_str(),
        "Duration" | "StdDuration" | "ChronoDuration"
    ) && constructor.ident.to_string().starts_with("from_")
        && call
            .args
            .iter()
            .any(|argument| matches!(argument, syn::Expr::Lit(_)))
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

fn has_test_attr(attrs: &[syn::Attribute]) -> bool {
    attrs
        .iter()
        .any(|attribute| attribute.path().is_ident("test"))
}

fn cap_shaped(name: &str, ty: &syn::Type) -> bool {
    if type_ends_with_duration(ty) {
        return true;
    }
    if !numeric_type(ty) {
        return false;
    }
    const TERMS: &[&str] = &[
        "TIMEOUT",
        "DEADLINE",
        "INTERVAL",
        "LIMIT",
        "MAX_",
        "_MAX",
        "CAPACITY",
        "RETRY",
        "ATTEMPT",
        "BATCH",
        "BUFFER",
        "CHUNK",
        "STALE",
        "GRACE",
        "TTL",
        "WINDOW",
        "THRESHOLD",
        "COOLDOWN",
        "PERIOD",
        "RESERVE",
        "RETAINED",
        "KEEP_ROWS",
        "DEPTH",
        "CEILING",
        "MAX_TURNS",
        "MAX_BYTES",
        "MAX_ROWS",
        "MAX_SESSIONS",
        "LIFETIME",
        "BOUND",
        "DELAY",
        "SAMPLE",
    ];
    TERMS.iter().any(|term| name.contains(term))
}

fn numeric_type(ty: &syn::Type) -> bool {
    let syn::Type::Path(path) = ty else {
        return false;
    };
    path.path.segments.last().is_some_and(|segment| {
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
        )
    })
}

fn type_ends_with_duration(ty: &syn::Type) -> bool {
    let syn::Type::Path(path) = ty else {
        return false;
    };
    path.path.segments.last().is_some_and(|segment| {
        matches!(
            segment.ident.to_string().as_str(),
            "Duration" | "StdDuration" | "ChronoDuration"
        )
    })
}

fn contains_literal_duration(expression: &syn::Expr) -> bool {
    struct DurationVisitor(bool);
    impl<'ast> Visit<'ast> for DurationVisitor {
        fn visit_expr_call(&mut self, call: &'ast syn::ExprCall) {
            if let syn::Expr::Path(path) = call.func.as_ref()
                && path
                    .path
                    .segments
                    .iter()
                    .any(|segment| segment.ident.to_string().starts_with("from_"))
                && call
                    .args
                    .iter()
                    .any(|argument| matches!(argument, syn::Expr::Lit(_)))
            {
                self.0 = true;
            }
            syn::visit::visit_expr_call(self, call);
        }
    }
    let mut visitor = DurationVisitor(false);
    visitor.visit_expr(expression);
    visitor.0
}

#[test]
fn production_cap_declarations_are_owned_and_registry_sites_resolve() {
    let root = workspace();
    let entries = registry();
    let registered: BTreeSet<_> = entries
        .iter()
        .filter(|cap| cap.source.ends_with(".rs"))
        .map(|cap| (cap.source.as_str(), cap.symbol.as_str()))
        .collect();
    let mut discovered = BTreeSet::new();
    let mut raw = Vec::new();

    for path in rust_sources(&root) {
        let relative = path
            .strip_prefix(&root)
            .unwrap()
            .to_string_lossy()
            .replace('\\', "/");
        let source = fs::read_to_string(&path).unwrap();
        let syntax = syn::parse_file(&source).unwrap_or_else(|error| {
            panic!("failed to parse {relative} while checking runtime caps: {error}")
        });
        let mut visitor = CapVisitor::default();
        visitor.visit_file(&syntax);
        discovered.extend(
            visitor
                .symbols
                .into_iter()
                .map(|symbol| (relative.clone(), symbol)),
        );
        raw.extend(
            visitor
                .raw_terminal_durations
                .into_iter()
                .map(|method| format!("{relative}: direct literal passed to {method}")),
        );
        raw.extend(
            visitor
                .raw_duration_literals
                .into_iter()
                .map(|function| format!("{relative}: direct Duration literal in {function}")),
        );
        raw.extend(
            visitor
                .raw_resource_bounds
                .into_iter()
                .map(|site| format!("{relative}: direct resource bound: {site}")),
        );
    }

    let missing: Vec<_> = discovered
        .iter()
        .filter(|(source, symbol)| !registered.contains(&(source.as_str(), symbol.as_str())))
        .cloned()
        .collect();
    let stale: Vec<_> = registered
        .iter()
        .filter(|(source, symbol)| !discovered.contains(&(source.to_string(), symbol.to_string())))
        .copied()
        .collect();
    assert!(
        missing.is_empty(),
        "production caps missing registry ownership/classification:\n{}",
        missing
            .iter()
            .map(|(source, symbol)| format!("  {source}#{symbol}"))
            .collect::<Vec<_>>()
            .join("\n")
    );
    assert!(
        stale.is_empty(),
        "registry entries without matching production call sites:\n{}",
        stale
            .iter()
            .map(|(source, symbol)| format!("  {source}#{symbol}"))
            .collect::<Vec<_>>()
            .join("\n")
    );
    assert!(
        raw.is_empty(),
        "production cap literals must use named registered declarations:\n{}",
        raw.join("\n")
    );
}

#[test]
fn shipped_script_caps_are_owned() {
    let root = workspace();
    let registered: BTreeSet<_> = registry()
        .iter()
        .filter(|cap| cap.source.starts_with("scripts/"))
        .map(|cap| (cap.source.as_str(), cap.symbol.as_str()))
        .collect();
    let candidates = [
        "anthropic-usage",
        "chatgpt-usage",
        "claude-code-cwd",
        "claude-code-locate-transcript",
        "claude-code-turns",
        "codex-cwd",
        "codex-locate-transcript",
        "codex-turns",
        "opencode-cwd",
        "opencode-turns",
        "zai-usage",
    ]
    .map(|name| root.join("scripts").join(name));
    let terms = [
        "TIMEOUT", "DEADLINE", "MAX_", "_LIMIT", "INTERVAL", "RETRY", "ATTEMPT",
    ];
    let mut missing = Vec::new();
    let mut discovered = BTreeSet::new();
    for path in candidates {
        let relative = path
            .strip_prefix(&root)
            .unwrap()
            .to_string_lossy()
            .replace('\\', "/");
        for line in fs::read_to_string(path).unwrap().lines() {
            let Some((left, _)) = line.split_once('=') else {
                continue;
            };
            let symbol = left.trim();
            if symbol
                .bytes()
                .all(|byte| byte == b'_' || byte.is_ascii_uppercase())
                && terms.iter().any(|term| symbol.contains(term))
            {
                discovered.insert((relative.clone(), symbol.to_string()));
                if !registered.contains(&(relative.as_str(), symbol)) {
                    missing.push(format!("{relative}#{symbol}"));
                }
            }
        }
    }
    assert!(
        missing.is_empty(),
        "shipped script caps missing registry entries:\n{}",
        missing.join("\n")
    );
    let stale = registered
        .iter()
        .filter(|(source, symbol)| !discovered.contains(&(source.to_string(), symbol.to_string())))
        .map(|(source, symbol)| format!("{source}#{symbol}"))
        .collect::<Vec<_>>();
    assert!(
        stale.is_empty(),
        "script registry entries without a shipped call site:\n{}",
        stale.join("\n")
    );
}

#[test]
fn invalid_and_unowned_registry_entries_are_rejected() {
    let valid = RuntimeCap {
        id: "owner.cap".into(),
        owner: "owner/module".into(),
        class: RuntimeCapClass::ResourceGuard,
        protected_resource: "memory".into(),
        exhaustion_behavior: "reject".into(),
        observability: "typed error".into(),
        configurability: "fixed".into(),
        rationale: "prevents unbounded allocation".into(),
        default_value: "1 byte".into(),
        source: "crates/example/src/lib.rs".into(),
        symbol: "MAX_BYTES".into(),
    };
    assert!(validate_registry(&[valid.clone()]).is_ok());
    let mut invalid = valid.clone();
    invalid.id = "Unstable ID".into();
    assert!(validate_registry(&[invalid]).is_err());
    let mut unowned = valid;
    unowned.owner.clear();
    assert!(validate_registry(&[unowned]).is_err());
}

#[test]
fn registry_class_inventory_is_queryable_and_stable() {
    let mut counts = BTreeMap::new();
    for cap in registry() {
        *counts.entry(cap.class).or_insert(0usize) += 1;
    }
    assert!(!counts.is_empty());
    assert!(registry().windows(2).all(|pair| pair[0].id < pair[1].id));
    let raw: serde_json::Value =
        serde_json::from_str(oulipoly_core::runtime_cap::registry_json()).unwrap();
    let raw_ids = raw["caps"]
        .as_array()
        .unwrap()
        .iter()
        .map(|cap| cap["id"].as_str().unwrap())
        .collect::<Vec<_>>();
    assert!(raw_ids.windows(2).all(|pair| pair[0] < pair[1]));
}
