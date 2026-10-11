//! Public configuration and legacy-constructor compatibility.

use std::time::Duration;

use surge_acp::bridge::{AcpBridge, BridgeConfig, BridgeError, BridgeTimeouts};

#[test]
fn rejects_each_zero_capacity_and_timeout() {
    let invalid = [
        BridgeConfig::new().cmd_capacity(0),
        BridgeConfig::new().event_capacity(0),
        BridgeConfig::new().timeouts(BridgeTimeouts {
            handshake: Duration::ZERO,
            ..BridgeTimeouts::default()
        }),
        BridgeConfig::new().timeouts(BridgeTimeouts {
            shutdown: Duration::ZERO,
            ..BridgeTimeouts::default()
        }),
    ];
    for config in invalid {
        assert!(matches!(config.spawn(), Err(BridgeError::InvalidConfig)));
    }
}

#[tokio::test]
async fn legacy_constructors_remain_usable_and_shutdown() {
    let bridges = [
        AcpBridge::spawn(2, 4).unwrap(),
        AcpBridge::spawn_with_timeouts(2, 4, BridgeTimeouts::default()).unwrap(),
        AcpBridge::with_defaults().unwrap(),
        BridgeConfig::default().spawn().unwrap(),
    ];
    for bridge in bridges {
        bridge.shutdown().await.unwrap();
    }
}
