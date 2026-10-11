// SPDX-License-Identifier: GPL-3.0-or-later
// SPDX-FileCopyrightText: 2026 Russet contributors

//! `rpc_method!` and `rpc_notification!` (ARCHITECTURE §5.3). Seam file: lead-only after W1 merges.

/// Declares a JSON-RPC method: a zero-sized marker type that implements [`Method`](crate::Method) and
/// [`RpcMethod`](crate::RpcMethod).
///
/// ```text
/// rpc_method!(Marker, "ns.verb", Params => Result, Permission::X [, long_running] [, confirm = Typed]
///             [, permission_fn = path]);
/// ```
///
/// `permission_fn` names a `fn(&Params, &ConnInfo, &dyn Ownership) -> russet_core::Result<Permission>` that
/// overrides [`Method::permission`](crate::Method::permission) [D 66]. Params and Result must derive
/// `Serialize, Deserialize, JsonSchema` and `TS`.
#[macro_export]
macro_rules! rpc_method {
    (
        $(#[$meta:meta])*
        $marker:ident, $name:literal, $params:ty => $result:ty, Permission::$perm:ident
        $(, $($opt:tt)*)?
    ) => {
        $crate::__rpc_method!(
            @opts [$(#[$meta])* $marker, $name, $params, $result, $perm] [false] [Plain] []
            $($($opt)*)?
        );
    };
}

#[doc(hidden)]
#[macro_export]
macro_rules! __rpc_method {
    (@opts $head:tt [$lr:tt] [$confirm:ident] [$($pfn:tt)*] long_running $(, $($rest:tt)*)?) => {
        $crate::__rpc_method!(@opts $head [true] [$confirm] [$($pfn)*] $($($rest)*)?);
    };
    (@opts $head:tt [$lr:tt] [$confirm:ident] [$($pfn:tt)*] confirm = $new:ident $(, $($rest:tt)*)?) => {
        $crate::__rpc_method!(@opts $head [$lr] [$new] [$($pfn)*] $($($rest)*)?);
    };
    (@opts $head:tt [$lr:tt] [$confirm:ident] [$($pfn:tt)*] permission_fn = $new:path $(, $($rest:tt)*)?) => {
        $crate::__rpc_method!(@opts $head [$lr] [$confirm] [$new] $($($rest)*)?);
    };
    (
        @opts [$(#[$meta:meta])* $marker:ident, $name:literal, $params:ty, $result:ty, $perm:ident]
        [$lr:tt] [$confirm:ident] [$($pfn:path)?]
    ) => {
        $(#[$meta])*
        #[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Hash)]
        pub struct $marker;

        impl $crate::Method for $marker {
            const NAME: &'static str = $name;
            const PERMISSION: $crate::Permission = $crate::Permission::$perm;
            const CONFIRM: $crate::Confirm = $crate::Confirm::$confirm;
            const LONG_RUNNING: bool = $lr;
            type Params = $params;
            type Result = $result;
            $(
                fn permission(
                    p: &Self::Params,
                    c: &$crate::ConnInfo,
                    o: &dyn $crate::Ownership,
                ) -> $crate::__private::CoreResult<$crate::Permission> {
                    $pfn(p, c, o)
                }
            )?
        }

        impl $crate::RpcMethod for $marker {
            const INFO: $crate::MethodInfo = $crate::MethodInfo::of::<$marker>();
        }
    };
}

/// Declares a JSON-RPC notification: a zero-sized marker type that implements
/// [`NotificationMethod`](crate::NotificationMethod) and [`RpcNotification`](crate::RpcNotification).
///
/// ```text
/// rpc_notification!(Marker, "ns.event", Params);
/// ```
#[macro_export]
macro_rules! rpc_notification {
    ($(#[$meta:meta])* $marker:ident, $name:literal, $params:ty $(,)?) => {
        $(#[$meta])*
        #[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Hash)]
        pub struct $marker;

        impl $crate::NotificationMethod for $marker {
            const NAME: &'static str = $name;
            type Params = $params;
        }

        impl $crate::RpcNotification for $marker {
            const INFO: $crate::NotificationInfo = $crate::NotificationInfo::of::<$marker>();
        }
    };
}
