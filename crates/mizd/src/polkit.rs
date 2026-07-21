//! polkit authorization for the mutating D-Bus methods.
//!
//! mizd runs as root on the system bus; clients are unprivileged. Mutating
//! methods (Install/Remove/Upgrade) must be authorized against the
//! `org.archetype.miz1.install` action (auth_admin_keep); the db-refresh tier
//! (`org.archetype.miz1.refresh`) is allow_active. There is NO polkit crate, so
//! the check is a hand-rolled zbus proxy call to
//! `org.freedesktop.PolicyKit1.Authority.CheckAuthorization`, keyed on the
//! caller's unique bus name (subject kind "system-bus-name").
//!
//! The `CheckAuthorization` D-Bus round-trip is VM-only (no polkit daemon on
//! the dev host). The pure pieces — action-id selection and the subject payload
//! shape — are unit-tested here.

use std::collections::HashMap;
use zbus::zvariant::Value;
use zbus::Connection;

/// polkit action ids. `REFRESH` is the allow_active read/refresh tier;
/// `INSTALL` is the auth_admin_keep mutation tier.
pub const ACTION_REFRESH: &str = "org.archetype.miz1.refresh";
pub const ACTION_INSTALL: &str = "org.archetype.miz1.install";

/// The mutating methods share the privileged install action.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Action {
    Refresh,
    Install,
}

impl Action {
    pub fn id(self) -> &'static str {
        match self {
            Action::Refresh => ACTION_REFRESH,
            Action::Install => ACTION_INSTALL,
        }
    }
}

/// Proxy to the polkit Authority. Signature mirrors the verified polkit D-Bus
/// surface; a wrong signature compiles but fails at runtime, so it is literal.
///
/// `CheckAuthorization(in (sa{sv}) subject, in s action_id,
///  in a{ss} details, in u flags, in s cancellation_id)
///  -> (bba{ss}) (is_authorized, is_challenge, details)`.
#[zbus::proxy(
    interface = "org.freedesktop.PolicyKit1.Authority",
    default_service = "org.freedesktop.PolicyKit1",
    default_path = "/org/freedesktop/PolicyKit1/Authority"
)]
trait Authority {
    #[allow(clippy::type_complexity)]
    fn check_authorization(
        &self,
        subject: &(&str, HashMap<&str, Value<'_>>),
        action_id: &str,
        details: HashMap<&str, &str>,
        flags: u32,
        cancellation_id: &str,
    ) -> zbus::Result<(bool, bool, HashMap<String, String>)>;
}

/// Build the polkit subject for a caller identified by its unique bus name
/// (subject kind "system-bus-name"). Returned as the `(s, a{sv})` tuple
/// `CheckAuthorization` expects.
fn bus_name_subject(sender: &str) -> (&'static str, HashMap<&'static str, Value<'_>>) {
    let mut attrs: HashMap<&'static str, Value<'_>> = HashMap::new();
    attrs.insert("name", Value::from(sender.to_string()));
    ("system-bus-name", attrs)
}

/// Check `action` for the D-Bus `sender` (the caller's unique bus name from the
/// method header). Returns `AccessDenied` on denial or on any polkit error.
///
/// VM-only: the `CheckAuthorization` call needs a running polkit daemon, absent
/// on the dev host. `AllowUserInteraction` (flags = 1) so `auth_admin_keep` can
/// prompt an admin for the mutation tier.
pub async fn check(conn: &Connection, sender: &str, action: Action) -> zbus::fdo::Result<()> {
    // Fail closed: an infra error (no polkit proxy, CheckAuthorization RPC
    // failure) is treated as a DENIAL (AccessDenied), never a generic Failed.
    let authority = AuthorityProxy::new(conn)
        .await
        .map_err(|e| zbus::fdo::Error::AccessDenied(format!("polkit proxy: {e}")))?;
    let subject = bus_name_subject(sender);
    let (authorized, _challenge, _details) = authority
        .check_authorization(&subject, action.id(), HashMap::new(), 1, "")
        .await
        .map_err(|e| zbus::fdo::Error::AccessDenied(format!("polkit CheckAuthorization: {e}")))?;
    if authorized {
        Ok(())
    } else {
        Err(zbus::fdo::Error::AccessDenied(format!(
            "not authorized for {}",
            action.id()
        )))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn action_ids_match_the_policy_actions() {
        assert_eq!(Action::Refresh.id(), "org.archetype.miz1.refresh");
        assert_eq!(Action::Install.id(), "org.archetype.miz1.install");
    }

    #[test]
    fn subject_is_system_bus_name_keyed_by_name() {
        let (kind, attrs) = bus_name_subject(":1.42");
        assert_eq!(kind, "system-bus-name");
        assert_eq!(attrs.len(), 1);
        assert_eq!(attrs.get("name"), Some(&Value::from(":1.42".to_string())));
    }
}
