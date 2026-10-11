// SPDX-License-Identifier: GPL-3.0-or-later
// SPDX-FileCopyrightText: 2026 Russet contributors

//! The method and notification registry (ARCHITECTURE §5.3, §7.7). Seam file: lead-only after W1 merges.
//!
//! [`registry()`] concatenates the per-module `METHODS` arrays in the fixed module order of `lib.rs`, so area
//! workstreams edit only their own namespace module. The capabilities list, the MCP tool mapping, the docs
//! and the schema and TypeScript export (`cargo xtask schema`, W2) all read it.

use std::sync::LazyLock;

use russet_core::error::{Error, codes};
use schemars::{JsonSchema, Schema, SchemaGenerator};
use serde::Deserialize;
use ts_rs::TS;

use crate::{Confirm, ConnInfo, Method, NotificationMethod, Ownership, Permission};

/// Schema and TypeScript hooks of one DTO type, so generators can work from the registry alone.
#[derive(Clone, Copy)]
pub struct TypeHooks {
    /// The type's JSON Schema; definitions go into the generator.
    pub schema: fn(&mut SchemaGenerator) -> Schema,
    /// The TypeScript name of the type.
    pub ts_name: fn(&ts_rs::Config) -> String,
    /// Exports the type and its dependencies as TypeScript files.
    pub ts_export_all: fn(&ts_rs::Config) -> Result<(), ts_rs::ExportError>,
}

impl TypeHooks {
    /// The hooks of `T`.
    pub const fn of<T: JsonSchema + TS + 'static>() -> Self {
        Self { schema: schema_of::<T>, ts_name: <T as TS>::name, ts_export_all: <T as TS>::export_all }
    }
}

impl std::fmt::Debug for TypeHooks {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str("TypeHooks")
    }
}

fn schema_of<T: JsonSchema>(g: &mut SchemaGenerator) -> Schema {
    g.subschema_for::<T>()
}

/// The per-call class of a method from its raw params [D 66]: [`Method::permission`] behind a type-erased
/// call, so the router and the permission engine need no concrete method type.
pub type PermissionOf = fn(&serde_json::Value, &ConnInfo, &dyn Ownership) -> russet_core::Result<Permission>;

/// Static description of a request method.
#[derive(Debug, Clone, Copy)]
pub struct MethodInfo {
    pub name: &'static str,
    /// The ceiling class (ARCHITECTURE §7.5, §8 "Class").
    pub permission: Permission,
    /// Whether the class is resolved per call: the §8 rows written "x (cond → y)" or "owner or system",
    /// declared with `rpc_method!(…, permission_fn = …)`. [`Self::permission`] is then the ceiling.
    pub dynamic_permission: bool,
    /// The class of one call (ARCHITECTURE §7.5). Static rows return [`Self::permission`] without reading the
    /// params. Dynamic rows deserialize them and call [`Method::permission`]: params that do not deserialize
    /// give `core.invalidArgument` (the IPC layer answers JSON-RPC `-32602` for them, ARCHITECTURE §13.1), and a
    /// row may refuse with `perm.denied` (`app.uninstall` from MCP).
    pub permission_of: PermissionOf,
    pub confirm: Confirm,
    pub long_running: bool,
    pub params: TypeHooks,
    pub result: TypeHooks,
}

impl MethodInfo {
    /// The description of `M`, whose class is [`Method::PERMISSION`] on every call; `rpc_method!` stores it as
    /// [`RpcMethod::INFO`].
    pub const fn of<M>() -> Self
    where
        M: Method,
        M::Params: TS,
        M::Result: TS,
    {
        Self {
            name: M::NAME,
            permission: M::PERMISSION,
            dynamic_permission: false,
            permission_of: ceiling_of::<M>,
            confirm: M::CONFIRM,
            long_running: M::LONG_RUNNING,
            params: TypeHooks::of::<M::Params>(),
            result: TypeHooks::of::<M::Result>(),
        }
    }

    /// The description of `M`, whose [`Method::permission`] depends on params or ownership;
    /// `rpc_method!(…, permission_fn = …)` stores it as [`RpcMethod::INFO`].
    pub const fn dynamic<M>() -> Self
    where
        M: Method,
        M::Params: TS,
        M::Result: TS,
    {
        Self { dynamic_permission: true, permission_of: per_call_permission_of::<M>, ..Self::of::<M>() }
    }
}

fn ceiling_of<M: Method>(
    _params: &serde_json::Value,
    _c: &ConnInfo,
    _o: &dyn Ownership,
) -> russet_core::Result<Permission> {
    Ok(M::PERMISSION)
}

fn per_call_permission_of<M: Method>(
    params: &serde_json::Value,
    conn: &ConnInfo,
    ownership: &dyn Ownership,
) -> russet_core::Result<Permission> {
    let params = M::Params::deserialize(params)
        .map_err(|e| Error::new(codes::core::INVALID_ARGUMENT, format!("invalid params for {}: {e}", M::NAME)))?;
    M::permission(&params, conn, ownership)
}

/// Static description of a notification.
#[derive(Debug, Clone, Copy)]
pub struct NotificationInfo {
    pub name: &'static str,
    pub params: TypeHooks,
}

impl NotificationInfo {
    /// The description of `N`; `rpc_notification!` stores it as [`RpcNotification::INFO`].
    pub const fn of<N>() -> Self
    where
        N: NotificationMethod,
        N::Params: TS,
    {
        Self { name: N::NAME, params: TypeHooks::of::<N::Params>() }
    }
}

/// A method declared with `rpc_method!`.
pub trait RpcMethod: Method {
    const INFO: MethodInfo;
}

/// A notification declared with `rpc_notification!`.
pub trait RpcNotification: NotificationMethod {
    const INFO: NotificationInfo;
}

/// One namespace module and its three arrays.
#[derive(Debug, Clone, Copy)]
pub struct Namespace {
    /// The module name; `flutter_build` and `flutter_run` both carry the `flutter` namespace.
    pub module: &'static str,
    /// Methods the daemon serves.
    pub methods: &'static [MethodInfo],
    /// Notifications the daemon sends.
    pub notifications: &'static [NotificationInfo],
    /// Requests the daemon sends to clients (`ui.*`, `ide.*`; ARCHITECTURE §9.3–§9.4).
    pub client_methods: &'static [MethodInfo],
}

/// Every namespace module, in the fixed order of `lib.rs`.
pub fn namespaces() -> &'static [Namespace] {
    crate::NAMESPACES
}

/// Every method the daemon serves (`capabilities.methods`, MCP mapping, docs, schemas).
pub fn registry() -> &'static [MethodInfo] {
    static ALL: LazyLock<Vec<MethodInfo>> =
        LazyLock::new(|| namespaces().iter().flat_map(|n| n.methods.iter().copied()).collect());
    &ALL
}

/// Every notification the daemon sends (`capabilities.notifications`).
pub fn notifications() -> &'static [NotificationInfo] {
    static ALL: LazyLock<Vec<NotificationInfo>> =
        LazyLock::new(|| namespaces().iter().flat_map(|n| n.notifications.iter().copied()).collect());
    &ALL
}

/// Every server-to-client request (`ui.*`, `ide.*`).
pub fn client_methods() -> &'static [MethodInfo] {
    static ALL: LazyLock<Vec<MethodInfo>> =
        LazyLock::new(|| namespaces().iter().flat_map(|n| n.client_methods.iter().copied()).collect());
    &ALL
}

/// The wire namespace a module carries.
pub fn namespace_of_module(module: &str) -> &str {
    match module {
        "flutter_build" | "flutter_run" => "flutter",
        other => other,
    }
}

#[cfg(test)]
mod tests {
    use std::collections::BTreeSet;

    use schemars::generate::SchemaSettings;
    use serde_json::Value;

    use super::*;

    fn is_lower_camel(seg: &str) -> bool {
        seg.chars().next().is_some_and(|c| c.is_ascii_lowercase()) && seg.chars().all(|c| c.is_ascii_alphanumeric())
    }

    #[test]
    fn every_namespace_file_is_in_the_table() {
        // The run-time CARGO_MANIFEST_DIR (cargo test sets it) names the tree being tested; the compiled-in one
        // is only the fallback (shared target slots, AGENTS.md §6).
        let manifest_dir = std::env::var_os("CARGO_MANIFEST_DIR")
            .map(std::path::PathBuf::from)
            .unwrap_or_else(|| std::path::PathBuf::from(env!("CARGO_MANIFEST_DIR")));
        let src = manifest_dir.join("src");
        let mut files: Vec<String> = std::fs::read_dir(src)
            .expect("src/ is readable")
            .filter_map(|e| e.ok()?.file_name().into_string().ok())
            .filter_map(|f| f.strip_suffix(".rs").map(str::to_owned))
            .filter(|m| !matches!(m.as_str(), "lib" | "registry" | "macros"))
            .collect();
        files.sort();
        let mut table: Vec<String> = namespaces().iter().map(|n| n.module.to_owned()).collect();
        table.sort();
        assert_eq!(files, table, "lib.rs's `pub mod` lines and its namespace table disagree");
    }

    #[test]
    fn names_are_unique() {
        let mut seen = BTreeSet::new();
        for m in registry().iter().chain(client_methods()) {
            assert!(seen.insert(m.name), "duplicate method {}", m.name);
        }
        let mut seen = BTreeSet::new();
        for n in notifications() {
            assert!(seen.insert(n.name), "duplicate notification {}", n.name);
        }
    }

    #[test]
    fn names_follow_the_conventions_and_their_module() {
        // ARCHITECTURE §7.3: `namespace.verb` or `namespace.sub.verb`, camelCase segments.
        for ns in namespaces() {
            let wire = namespace_of_module(ns.module);
            let names = ns
                .methods
                .iter()
                .chain(ns.client_methods)
                .map(|m| m.name)
                .chain(ns.notifications.iter().map(|n| n.name));
            for name in names {
                if name == "initialize" {
                    assert_eq!(ns.module, "initialize");
                    continue;
                }
                let segs: Vec<&str> = name.split('.').collect();
                assert!(segs.len() >= 2, "{name} has no namespace");
                assert_eq!(segs[0], wire, "{name} is declared in module {}", ns.module);
                assert!(segs.iter().all(|s| is_lower_camel(s)), "{name} is not camelCase");
            }
        }
    }

    #[test]
    fn confirmation_and_classes_are_consistent() {
        for m in registry() {
            let typed = m.confirm == Confirm::Typed;
            let destructive = m.permission == Permission::AccountDestructive;
            assert_eq!(typed, destructive || m.name == "ship.review.submit", "{}: Confirm::Typed rule", m.name);
        }
        for m in client_methods() {
            assert_eq!(m.permission, Permission::None, "{}: client-side requests carry no class", m.name);
        }
    }

    struct NoOwnership;

    impl Ownership for NoOwnership {
        fn owns_job(&self, _conn: &str, _job: &crate::JobId) -> bool {
            false
        }
        fn owns_lease(&self, _conn: &str, _lease: &crate::LeaseId) -> bool {
            false
        }
        fn project_bundle_ids(&self, _conn: &str) -> Vec<String> {
            Vec::new()
        }
    }

    fn conn(kind: crate::ClientKind) -> ConnInfo {
        ConnInfo { id: "c-1".into(), kind, name: "test".into(), interactive: false, shared_lease: None }
    }

    #[test]
    fn the_dynamic_rows_are_the_documented_ones() {
        // ARCHITECTURE §5.3 `Method::permission` and the §8 rows written "x (cond → y)" or "owner or system".
        let dynamic: BTreeSet<&str> = registry().iter().filter(|m| m.dynamic_permission).map(|m| m.name).collect();
        let expected = BTreeSet::from([
            "app.uninstall",
            "config.set",
            "config.unset",
            "doctor.run",
            "job.cancel",
            "lease.release",
            "screen.mirror.start",
        ]);
        assert_eq!(dynamic, expected);
    }

    #[test]
    fn the_per_call_class_is_reachable_through_the_registry() {
        let find = |name: &str| registry().iter().find(|m| m.name == name).copied().expect(name);
        let cli = conn(crate::ClientKind::Cli);
        // Static rows answer their ceiling without reading the params.
        let list = find("device.list");
        assert_eq!((list.permission_of)(&serde_json::json!(7), &cli, &NoOwnership).ok(), Some(Permission::Read));
        // Dynamic rows deserialize the params and call Method::permission.
        let doctor = find("doctor.run");
        assert!((doctor.permission_of)(&serde_json::json!({}), &cli, &NoOwnership).is_ok());
        let bad = (doctor.permission_of)(&serde_json::json!(7), &cli, &NoOwnership).map_err(|e| e.code());
        assert_eq!(bad, Err(codes::core::INVALID_ARGUMENT));
        let uninstall = find("app.uninstall");
        let mcp = conn(crate::ClientKind::Mcp);
        let denied = (uninstall.permission_of)(&serde_json::json!({}), &mcp, &NoOwnership).map_err(|e| e.code());
        assert_eq!(denied, Err(codes::perm::DENIED));
        assert_eq!(
            (uninstall.permission_of)(&serde_json::json!({}), &cli, &NoOwnership).ok(),
            Some(Permission::Device)
        );
    }

    #[test]
    fn catalog_spot_checks() {
        let find = |name: &str| registry().iter().find(|m| m.name == name).copied();
        let init = find("initialize").expect("initialize is registered");
        assert_eq!(init.permission, Permission::None);
        let revoke = find("portal.certificates.revoke").expect("revoke is registered");
        assert_eq!((revoke.permission, revoke.confirm), (Permission::AccountDestructive, Confirm::Typed));
        assert!(find("build.start").expect("build.start").long_running);
        assert!(!find("device.list").expect("device.list").long_running);
        assert_eq!(find("ship.upload").expect("ship.upload").permission, Permission::Publish);
        assert!(client_methods().iter().any(|m| m.name == "ui.confirm"));
        assert!(notifications().iter().any(|n| n.name == "job.updated"));
        assert!(registry().len() >= 190, "only {} methods registered", registry().len());
    }

    /// The schema of every method and notification DTO, with definitions inlined into `$defs`.
    fn all_schemas() -> Vec<(String, Value)> {
        let mut out = Vec::new();
        let mut generator = SchemaSettings::draft2020_12().into_generator();
        let mut push = |name: String, hooks: &TypeHooks, generator: &mut SchemaGenerator| {
            let s = (hooks.schema)(generator);
            out.push((name, s.to_value()));
        };
        for m in registry().iter().chain(client_methods()) {
            push(format!("{} params", m.name), &m.params, &mut generator);
            push(format!("{} result", m.name), &m.result, &mut generator);
        }
        for n in notifications() {
            push(format!("{} params", n.name), &n.params, &mut generator);
        }
        for (name, def) in generator.definitions() {
            out.push((format!("$defs/{name}"), def.clone()));
        }
        out
    }

    /// Paths of externally tagged enum variants (`{"Variant": …}` objects) inside `v`.
    fn externally_tagged(v: &Value, path: &str, found: &mut Vec<String>) {
        match v {
            Value::Object(map) => {
                for key in ["oneOf", "anyOf"] {
                    if let Some(Value::Array(variants)) = map.get(key) {
                        for (i, variant) in variants.iter().enumerate() {
                            let props = variant.get("properties").and_then(Value::as_object);
                            let required = variant.get("required").and_then(Value::as_array);
                            if let (Some(props), Some(required)) = (props, required)
                                && props.len() == 1
                                && required.len() == 1
                                && !props.contains_key("kind")
                                && variant.get("additionalProperties") == Some(&Value::Bool(false))
                            {
                                found.push(format!("{path}/{key}/{i}"));
                            }
                        }
                    }
                }
                for (k, child) in map {
                    externally_tagged(child, &format!("{path}/{k}"), found);
                }
            }
            Value::Array(items) => {
                for (i, child) in items.iter().enumerate() {
                    externally_tagged(child, &format!("{path}/{i}"), found);
                }
            }
            _ => {}
        }
    }

    #[test]
    fn wire_enums_are_never_externally_tagged() {
        // ARCHITECTURE §5.2 [D 72]: unit enums are kebab-case strings; data-carrying enums are internally
        // tagged with `kind`. Externally tagged enums are rejected.
        let mut found = Vec::new();
        for (name, schema) in all_schemas() {
            externally_tagged(&schema, &name, &mut found);
        }
        assert!(found.is_empty(), "externally tagged enums on the wire: {found:?}");
    }

    #[test]
    fn the_wire_enum_check_catches_an_externally_tagged_enum() {
        #[derive(schemars::JsonSchema)]
        #[allow(dead_code)]
        enum Bad {
            Tuple(u32),
            Struct { x: u32 },
        }
        #[derive(schemars::JsonSchema)]
        #[serde(tag = "kind", rename_all = "kebab-case")]
        #[allow(dead_code)]
        enum Good {
            Unit,
            Struct { x: u32 },
        }
        let mut generator = SchemaSettings::draft2020_12().into_generator();
        let bad = generator.root_schema_for::<Bad>().to_value();
        let good = generator.root_schema_for::<Good>().to_value();
        let mut found = Vec::new();
        externally_tagged(&bad, "Bad", &mut found);
        assert_eq!(found.len(), 2, "both Bad variants are externally tagged: {found:?}");
        let mut found = Vec::new();
        externally_tagged(&good, "Good", &mut found);
        assert!(found.is_empty(), "Good is internally tagged: {found:?}");
    }
}
