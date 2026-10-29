use std::process::Command;

#[test]
fn missing_required_measurement_returns_failure_and_writes_a_report() {
    let root = std::env::temp_dir().join(format!("n2p-quality-cli-{}", std::process::id()));
    std::fs::create_dir_all(&root).unwrap();
    let config = root.join("config.json");
    let report = root.join("report.json");
    std::fs::write(&config,r#"{"version":1,"provenance":{},"checks":[{"name":"missing bounds","category":"placement","metric":{"kind":"unmeasured","reason":"No PowerPoint measurements"}}]}"#).unwrap();
    let result = Command::new(env!("CARGO_BIN_EXE_xtask"))
        .args(["quality-eval", "--config"])
        .arg(&config)
        .arg("--report")
        .arg(&report)
        .output()
        .unwrap();
    assert!(!result.status.success());
    let value: serde_json::Value =
        serde_json::from_slice(&std::fs::read(&report).unwrap()).unwrap();
    assert_eq!(value["summary"]["required_failures"], 1);
    assert_eq!(value["checks"][0]["status"], "unmeasured");
    std::fs::remove_dir_all(root).unwrap();
}
