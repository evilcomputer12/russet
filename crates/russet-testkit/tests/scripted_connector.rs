// SPDX-License-Identifier: AGPL-3.0-only
// SPDX-FileCopyrightText: 2026 Russet contributors

//! The scripted device backend (CORE-15): attach and detach, facade calls, scripted streams, syslog lines and
//! failures, without a phone (PLAN W8; ARCHITECTURE §3.3 item 7).

use std::sync::Arc;
use std::time::Duration;

use futures::StreamExt;
use russet_core::error::codes;
use russet_device::io::{FacadeCall, ServiceConnector, live_connectors_constructed};
use russet_device::{DeviceEvent, Udid};
use russet_testkit::scenario::{FailOp, Scenario, default_scenario_path};
use russet_testkit::scripted::{Op, SYSLOG_RELAY};
use russet_testkit::{Control, ScriptedConnector};
use serde_json::json;
use tokio::io::{AsyncReadExt, AsyncWriteExt};

const UDID: &str = "00000000-0000000000000001";

fn udid() -> Udid {
    Udid(UDID.into())
}

fn call(api: &str, args: serde_json::Value) -> FacadeCall {
    FacadeCall { api: api.into(), args }
}

async fn next_event(s: &mut futures::stream::BoxStream<'static, DeviceEvent>) -> DeviceEvent {
    tokio::time::timeout(Duration::from_secs(5), s.next()).await.expect("an event in time").expect("an open stream")
}

fn scripted(text: &str) -> (ScriptedConnector, Control) {
    let c = ScriptedConnector::new(Scenario::from_toml(text).expect("scenario"));
    let ctl = c.control();
    (c, ctl)
}

#[test]
fn the_default_scenario_file_is_the_embedded_one_and_is_valid() {
    let from_file = Scenario::from_file(&default_scenario_path()).expect("default.toml");
    assert_eq!(from_file, Scenario::default_scenario());
    let d = &from_file.devices[0];
    assert_eq!((d.udid.as_str(), d.product_type.as_str(), d.os_version.as_str()), (UDID, "iPhone13,2", "27.0"));
    assert!(d.paired && d.developer_mode && !d.locked);
    assert_eq!(d.ddi.as_ref().and_then(|x| x.product_build.as_deref()), Some("27A266a"));
    assert!(
        ScriptedConnector::from_backend_spec(&format!("scripted:{}", default_scenario_path().display()))
            .expect("spec")
            .is_some()
    );
    assert!(ScriptedConnector::from_backend_spec("none").expect("spec").is_none());
    let missing = ScriptedConnector::from_backend_spec("scripted:/nonexistent/scenario.toml").expect_err("missing");
    assert_eq!(missing.code(), codes::core::NOT_FOUND);
}

#[tokio::test]
async fn watch_reports_attached_devices_and_runtime_detach_and_attach() {
    let c = ScriptedConnector::default_scenario();
    let ctl = c.control();
    let conn: Arc<dyn ServiceConnector> = Arc::new(c);
    let mut events = conn.watch().await.expect("watch");
    assert!(matches!(next_event(&mut events).await, DeviceEvent::Added(s) if s.udid == udid()));
    assert!(ctl.detach(UDID));
    assert_eq!(next_event(&mut events).await, DeviceEvent::Removed(udid()));
    let err = conn.call(call("lockdown.get_value", json!({ "udid": UDID, "key": "ProductVersion" }))).await;
    assert_eq!(err.expect_err("detached").code(), codes::device::DISCONNECTED);
    assert!(ctl.attach(UDID));
    assert!(matches!(next_event(&mut events).await, DeviceEvent::Added(s) if s.udid == udid()));
    assert!(!ctl.attach("00000000-00000000000000ff"), "unknown devices cannot be attached");
    assert_eq!(live_connectors_constructed(), 0, "the scripted backend never builds a LiveConnector");
}

#[tokio::test]
async fn the_timeline_attaches_and_detaches_devices() {
    let (c, ctl) = scripted(
        r#"
        schema = 1
        name = "timeline"
        [[device]]
        udid = "00000000-0000000000000002"
        attach-at-ms = 50
        detach-at-ms = 250
        "#,
    );
    let u = Udid("00000000-0000000000000002".into());
    assert!(!ctl.is_attached(&u.0), "not attached before attach-at-ms");
    let mut events = c.watch().await.expect("watch");
    assert!(matches!(next_event(&mut events).await, DeviceEvent::Added(s) if s.udid == u));
    assert!(ctl.is_attached(&u.0));
    assert_eq!(next_event(&mut events).await, DeviceEvent::Removed(u.clone()));
    assert!(!ctl.is_attached(&u.0));
}

#[tokio::test]
async fn lockdown_values_come_from_the_device() {
    let c = ScriptedConnector::default_scenario();
    let v = c.call(call("lockdown.get_value", json!({ "key": "ProductVersion" }))).await.expect("value");
    assert_eq!(v, json!("27.0"), "the only attached device is implied");
    let all = c.call(call("lockdown.get_value", json!({ "udid": UDID }))).await.expect("dict");
    assert_eq!(all["ProductType"], json!("iPhone13,2"));
    assert_eq!(all["BuildVersion"], json!("24A437"));
    assert_eq!(all["HardwareModel"], json!("D53gAP"), "[device.lockdown] values are served too");
    let dev_mode = c
        .call(call(
            "lockdown.get_value",
            json!({ "domain": "com.apple.security.mac.amfi", "key": "DeveloperModeStatus" }),
        ))
        .await
        .expect("amfi");
    assert_eq!(dev_mode, json!(true));
    let missing = c.call(call("lockdown.get_value", json!({ "key": "NoSuchKey" }))).await.expect_err("missing");
    assert_eq!(missing.code(), codes::device::PROTOCOL);
    let unknown = c.call(call("lockdown.get_value", json!({ "udid": "00000000-0000000000000009" }))).await;
    assert_eq!(unknown.expect_err("unknown device").code(), codes::device::NOT_FOUND);
}

#[tokio::test]
async fn scripted_calls_match_arguments_and_return_errors() {
    let (c, ctl) = scripted(
        r#"
        schema = 1
        name = "calls"
        [[device]]
        udid = "00000000-0000000000000001"
        [[call]]
        api = "installation_proxy.browse"
        args = { ApplicationType = "User" }
        result = [{ CFBundleIdentifier = "dev.russet.test.probe", CFBundleVersion = "1" }]
        [[call]]
        api = "installation_proxy.browse"
        result = []
        [[call]]
        api = "mobile_image_mounter.lookup"
        error = { code = "device.ddiNotMounted", message = "no DDI" }
        "#,
    );
    let user = c.call(call("installation_proxy.browse", json!({ "ApplicationType": "User", "extra": 1 }))).await;
    assert_eq!(user.expect("browse")[0]["CFBundleIdentifier"], json!("dev.russet.test.probe"));
    let any = c.call(call("installation_proxy.browse", json!({ "ApplicationType": "System" }))).await;
    assert_eq!(any.expect("fallback entry"), json!([]));
    let ddi = c.call(call("mobile_image_mounter.lookup", json!({}))).await.expect_err("scripted error");
    assert_eq!(ddi.code(), codes::device::DDI_NOT_MOUNTED);
    let unknown = c.call(call("springboard.icons", json!({}))).await.expect_err("not scripted");
    assert_eq!(unknown.code(), codes::device::SERVICE_NOT_IMPLEMENTED);
    assert!(unknown.hint().is_some_and(|h| h.contains("[[call]]")));
    assert_eq!(ctl.calls().len(), 4);
}

#[tokio::test]
async fn a_scripted_stream_plays_its_steps() {
    let (c, ctl) = scripted(
        r#"
        schema = 1
        name = "echo"
        [[device]]
        udid = "00000000-0000000000000001"
        [[service]]
        name = "com.example.echo"
        steps = [{ expect = "ping\n" }, { send = "pong\n" }, { send-b64 = "AAEC" }, { close = true }]
        "#,
    );
    let mut s = c.lockdown_service(&udid(), "com.example.echo").await.expect("stream");
    s.write_all(b"ping\n").await.expect("write");
    let mut got = Vec::new();
    s.read_to_end(&mut got).await.expect("read until close");
    assert_eq!(got, b"pong\n\x00\x01\x02");
    tokio::time::sleep(Duration::from_millis(20)).await;
    ctl.assert_consumed().expect("every step ran");
    let rsd = c.rsd_service(&udid(), "com.example.echo").await.expect_err("only scripted for lockdown");
    assert_eq!(rsd.code(), codes::device::SERVICE_UNAVAILABLE);
    assert_eq!(
        ctl.ops(),
        vec![
            Op::Lockdown { udid: UDID.into(), service: "com.example.echo".into() },
            Op::Rsd { udid: UDID.into(), service: "com.example.echo".into() },
        ]
    );
}

#[tokio::test]
async fn wrong_client_bytes_are_a_recorded_mismatch() {
    let (c, ctl) = scripted(
        r#"
        schema = 1
        name = "strict"
        [[device]]
        udid = "00000000-0000000000000001"
        [[service]]
        name = "com.example.strict"
        via = "rsd"
        steps = [{ expect = "hello" }, { send = "world" }]
        "#,
    );
    let mut s = c.rsd_service(&udid(), "com.example.strict").await.expect("stream");
    s.write_all(b"HELLO").await.expect("write");
    let mut got = Vec::new();
    let _ = s.read_to_end(&mut got).await;
    assert!(got.is_empty(), "nothing is sent after a mismatch");
    let mismatches = ctl.mismatches();
    assert_eq!(mismatches.len(), 1, "{mismatches:?}");
    assert!(mismatches[0].contains("expected \"hello\", the client sent \"HELLO\""), "{mismatches:?}");
    assert!(ctl.assert_consumed().is_err());
}

#[tokio::test]
async fn the_syslog_relay_speaks_idevices_framing() {
    let c = ScriptedConnector::default_scenario();
    let ctl = c.control();
    let stream = c.lockdown_service(&udid(), SYSLOG_RELAY).await.expect("syslog relay");
    // idevice's own client reads the scripted stream, as the live device plane would.
    let mut client = idevice::syslog_relay::SyslogRelayClient::new(idevice::Idevice::new(stream, "scripted"));
    for n in 1..=3 {
        let line = tokio::time::timeout(Duration::from_secs(5), client.next()).await.expect("in time").expect("line");
        assert!(line.ends_with(&format!("RussetProbe[932] <Notice>: PROBE seq={n}")), "{line}");
    }
    // Lines pushed by the test follow the scripted ones.
    let mut rest = Vec::new();
    for _ in 0..97 {
        rest.push(client.next().await.expect("scripted line"));
    }
    assert!(rest.last().is_some_and(|l| l.ends_with("PROBE seq=100")));
    assert_eq!(ctl.push_log(UDID, "pushed by the test"), 1);
    assert_eq!(client.next().await.expect("pushed line"), "pushed by the test");
}

#[tokio::test]
async fn a_detach_closes_open_streams() {
    let c = ScriptedConnector::default_scenario();
    let ctl = c.control();
    let mut s = c.lockdown_service(&udid(), SYSLOG_RELAY).await.expect("stream");
    ctl.detach(UDID);
    let mut sink = Vec::new();
    let read = tokio::time::timeout(Duration::from_secs(5), s.read_to_end(&mut sink)).await.expect("EOF in time");
    assert!(read.is_ok(), "the stream ends cleanly");
    let again = c.lockdown_service(&udid(), SYSLOG_RELAY).await.expect_err("detached");
    assert_eq!(again.code(), codes::device::DISCONNECTED);
}

#[tokio::test]
async fn scripted_and_injected_failures() {
    let (c, ctl) = scripted(
        r#"
        schema = 1
        name = "failures"
        [[device]]
        udid = "00000000-0000000000000001"
        [[service]]
        name = "com.apple.coredevice.appservice"
        via = "rsd"
        steps = [{ send = "ok" }]
        [[failure]]
        op = "rsd-service"
        name = "com.apple.coredevice.appservice"
        code = "device.tunnelFailed"
        times = 1
        "#,
    );
    let first = c.rsd_service(&udid(), "com.apple.coredevice.appservice").await.expect_err("first fails");
    assert_eq!(first.code(), codes::device::TUNNEL_FAILED);
    c.rsd_service(&udid(), "com.apple.coredevice.appservice").await.expect("then it works");
    ctl.fail_next(FailOp::Call, Some("lockdown.get_value"), codes::device::LOCKED, 2);
    for _ in 0..2 {
        let e = c.call(call("lockdown.get_value", json!({ "key": "DeviceName" }))).await.expect_err("locked");
        assert_eq!(e.code(), codes::device::LOCKED);
    }
    let name = c.call(call("lockdown.get_value", json!({ "key": "DeviceName" }))).await.expect("unlocked");
    assert_eq!(name, json!("Russet Test iPhone"));
    ctl.fail_next(FailOp::Watch, None, codes::device::USBMUX_UNAVAILABLE, 1);
    assert_eq!(c.watch().await.err().map(|e| e.code()), Some(codes::device::USBMUX_UNAVAILABLE));
}

#[test]
fn invalid_scenarios_are_rejected_with_a_reason() {
    let cases = [
        ("schema = 2\nname = \"x\"\n", "schema 2"),
        ("schema = 1\nname = \"x\"\n[[device]]\nudid = \"a\"\n[[device]]\nudid = \"a\"\n", "appears twice"),
        ("schema = 1\nname = \"x\"\n[[failure]]\nop = \"call\"\ncode = \"device.nope\"\n", "unknown error code"),
        ("schema = 1\nname = \"x\"\n[[call]]\napi = \"a\"\n", "exactly one of result and error"),
        (
            "schema = 1\nname = \"x\"\n[[service]]\nname = \"s\"\nsteps = [{ send = \"a\", expect = \"b\" }]\n",
            "exactly one of send",
        ),
        (
            "schema = 1\nname = \"x\"\n[[device]]\nudid = \"a\"\nattach-at-ms = 5\ndetach-at-ms = 5\n",
            "after attach-at-ms",
        ),
        ("schema = 1\nname = \"x\"\nunknown-key = 1\n", "unknown field"),
    ];
    for (text, why) in cases {
        let e = Scenario::from_toml(text).expect_err(why);
        assert_eq!(e.code(), codes::core::INVALID_ARGUMENT);
        assert!(e.message().contains(why), "{why}: {}", e.message());
    }
}
