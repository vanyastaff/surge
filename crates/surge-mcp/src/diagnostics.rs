//! Mandatory application tracing boundary for external MCP transport diagnostics.
//!
//! Apply this predicate as a global registry filter, before every output layer,
//! AND-composed with the application's ordinary level filter. Downstream library
//! consumers must apply the same boundary to their own subscriber: a scoped
//! dispatcher cannot contain transport tasks spawned by dependencies.

/// Whether a tracing target may publish through Surge's application subscriber.
///
/// External MCP transport namespaces can contain arbitrary child payloads in
/// events and spans, including handshake, notification and codec diagnostics.
/// The predicate uses metadata only; it never renders or visits their fields.
#[must_use]
pub fn permits_target(target: &str) -> bool {
    !["rmcp", "process_wrap"].iter().any(|namespace| {
        target == *namespace
            || target
                .strip_prefix(namespace)
                .is_some_and(|suffix| suffix.starts_with("::"))
    })
}

#[cfg(test)]
mod tests {
    use super::permits_target;

    #[test]
    fn vetoes_exact_transport_namespaces_without_hiding_nearby_targets() {
        for target in [
            "rmcp",
            "rmcp::service",
            "rmcp::transport::codec",
            "process_wrap",
            "process_wrap::tokio",
        ] {
            assert!(!permits_target(target), "{target}");
        }
        for target in [
            "",
            "rmcp_extra",
            "rmcp.service",
            "process_wrapper",
            "surge_mcp",
            "mcp::supervisor",
        ] {
            assert!(permits_target(target), "{target}");
        }
    }
}
