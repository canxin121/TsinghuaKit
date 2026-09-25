use super::*;

#[derive(Default)]
struct CountWriter {
    writes: usize,
    flushes: usize,
    bytes: Vec<u8>,
}
impl Write for CountWriter {
    fn write(&mut self, bytes: &[u8]) -> io::Result<usize> {
        self.writes += 1;
        self.bytes.extend_from_slice(bytes);
        Ok(bytes.len())
    }
    fn flush(&mut self) -> io::Result<()> {
        self.flushes += 1;
        Ok(())
    }
}

#[test]
fn backend_repair_request_audit_buffered_report_preserves_bytes_and_reduces_write_calls() {
    let value = serde_json::json!({"cases": (0..34).map(|i| serde_json::json!({
        "case": format!("fixture_{i}"), "status":"passed", "requests":3,
        "timing": {"phases": (0..10).map(|j| serde_json::json!({
            "phase":j,"total_us":1234,"p50_us":123,"p95_us":456,"p99_us":789,
            "count":10,"max_us":900,"min_us":20,
        })).collect::<Vec<_>>()}
    })).collect::<Vec<_>>()});
    let mut direct = CountWriter::default();
    serde_json::to_writer_pretty(&mut direct, &value).unwrap();
    direct.write_all(b"\n").unwrap();
    let mut buffered = CountWriter::default();
    write_json_buffered(&mut buffered, &value).unwrap();
    assert_eq!(direct.bytes, buffered.bytes);
    assert!(direct.writes > 1000);
    assert!(buffered.writes < direct.writes / 100);
    assert_eq!(buffered.flushes, 1);
}

struct SerializeFailure;
impl serde::Serialize for SerializeFailure {
    fn serialize<S: serde::Serializer>(&self, _: S) -> Result<S::Ok, S::Error> {
        Err(serde::ser::Error::custom("fixture serialization failure"))
    }
}

#[test]
fn backend_repair_request_audit_failed_buffered_checkpoint_keeps_previous_report_and_cleans_temp() {
    let root = std::env::temp_dir().join(format!("thyou-report-audit-{}", uuid::Uuid::new_v4()));
    let path = root.join("report.json");
    let previous = serde_json::json!({"status":"last_complete_fixture"});
    write_private_json(&path, &previous).unwrap();
    let before = std::fs::read(&path).unwrap();
    assert!(write_private_json(&path, &SerializeFailure).is_err());
    assert_eq!(std::fs::read(&path).unwrap(), before);
    assert_eq!(std::fs::read_dir(&root).unwrap().count(), 1);
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        assert_eq!(
            std::fs::metadata(&path).unwrap().permissions().mode() & 0o077,
            0
        );
    }
    let _ = std::fs::remove_dir_all(root);
}

#[test]
fn backend_repair_request_audit_buffered_output_propagates_flush_failure() {
    struct FailsOnFlush;
    impl Write for FailsOnFlush {
        fn write(&mut self, bytes: &[u8]) -> io::Result<usize> {
            Ok(bytes.len())
        }
        fn flush(&mut self) -> io::Result<()> {
            Err(io::Error::other("fixture flush failure"))
        }
    }
    assert!(write_json_buffered(&mut FailsOnFlush, &serde_json::json!({"value":1})).is_err());
}
