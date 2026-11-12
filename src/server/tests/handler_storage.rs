//! Storage for handlers: the server fetches a CID input's content for the
//! handler, and uploads content outputs and text too big for a chat message.

use super::*;
use logos_mcp::guard::FilesRoot;

const CID: &str = "zDvZRwzmDMMpY8Qd5LWZGWworQRVDHEZ22co3GX5B8of7k3oGrvU";
const OUT_CID: &str = "zDvZRwzmDMMpY8Qd5LWZGWworQRVDHEZ22co3GX5B8of7k3oGrvX";

/// A provider with a paid escrow job for `input`, an offer whose handler is
/// `url`, and a files root unless `files` is false.
struct Stored {
    s: AgentServer,
    fake: Arc<Routed>,
    events: Arc<EventStore>,
    dir: tempfile::TempDir,
    files: Option<tempfile::TempDir>,
}

fn storage(method: &str, value: &str) -> RawOutput {
    out(&format!(
        r#"{{"method":"{method}","module":"storage_module","result":{{"error":null,"success":true,"value":{value}}},"status":"ok"}}"#
    ))
}

async fn paid_storage_job(url: &str, input: Payload, files: bool) -> Stored {
    let request = proto(
        1,
        Body::Request {
            service: SVC.into(),
            input,
        },
    );
    let fake = node();
    fake.set(
        "chat_module.get_messages",
        provider_escrow_history_for(&request, FAR + 600_000),
    );
    fake.set(
        "lez_core.get_account_public",
        job_record(PROVIDER_ACCOUNT, AGENT_FROM, 1_000_000, FAR + 600_000, 0),
    );
    fake.set(
        "lez_core.send_generic_public_transaction",
        generic_ok("tx1"),
    );
    fake.set(
        "storage_module.downloadCancel",
        storage("downloadCancel", "null"),
    );
    fake.set(
        "storage_module.uploadCancel",
        storage("uploadCancel", "null"),
    );
    let dir = tempfile::tempdir().unwrap();
    let files = files.then(|| tempfile::tempdir().unwrap());
    let events = Arc::new(EventStore::default());
    let s = server_with(
        fake.clone(),
        dir.path(),
        files.as_ref().map(|f| FilesRoot::new(f.path()).unwrap()),
        events.clone(),
    );
    s.offer_spec(&spec(Some(url), 1)).await.unwrap();
    Stored {
        s,
        fake,
        events,
        dir,
        files,
    }
}

/// A storage event as storage_module emits it.
fn emit(events: &EventStore, event: &str, body: Value) {
    events.apply_event(event, &json!({ "arg0": body.to_string() }));
}

/// The file path a storage call was given.
fn path_in(args: &[String]) -> std::path::PathBuf {
    args.iter()
        .find_map(|a| a.strip_prefix("str:").filter(|p| p.starts_with('/')))
        .expect("a path argument")
        .into()
}

/// Answer the `n`th download storage_module was asked for: write `content`
/// where it was told to, then report it done.
async fn serve_download(st: &Stored, n: usize, content: &[u8]) {
    let args = calls(&st.fake, "storage_module.downloadToUrl", n).await[n - 1].clone();
    std::fs::write(path_in(&args), content).unwrap();
    let size = content.len();
    emit(
        &st.events,
        "storageDownloadProgress",
        json!({ "sessionId": CID, "bytes": size, "total": size, "success": true }),
    );
    emit(
        &st.events,
        "storageDownloadDone",
        json!({ "sessionId": CID, "success": true }),
    );
}

/// Wait for the `n`th upload, returning what it uploads, then report it done
/// under `session` with `outcome`.
async fn serve_upload(st: &Stored, n: usize, session: &str, outcome: Value) -> Vec<u8> {
    let args = calls(&st.fake, "storage_module.uploadUrl", n).await[n - 1].clone();
    let content = std::fs::read(path_in(&args)).unwrap();
    let mut done = json!({ "sessionId": session });
    done.as_object_mut()
        .unwrap()
        .extend(outcome.as_object().unwrap().clone());
    emit(&st.events, "storageUploadDone", done);
    content
}

/// The output kept for the job, as delivered.
async fn delivered(st: &Stored) -> Value {
    let sends = calls(&st.fake, "lez_core.send_generic_public_transaction", 1).await;
    assert_eq!(instruction(&sends[0])[0], 1, "a Deliver");
    let kept = std::fs::read_to_string(st.dir.path().join("service-results.json")).unwrap();
    serde_json::from_str::<Value>(&kept).unwrap()[JOB].clone()
}

/// The job's files the server left in the files root.
fn left_behind(st: &Stored) -> Vec<String> {
    let jobs = st.files.as_ref().unwrap().path().join("logos-agents-jobs");
    std::fs::read_dir(jobs)
        .expect("the server's job folder")
        .map(|e| e.unwrap().file_name().to_string_lossy().into_owned())
        .collect()
}

/// The reason a returned job was declined with.
async fn declined(st: &Stored) -> String {
    let sends = calls(&st.fake, "lez_core.send_generic_public_transaction", 1).await;
    assert_eq!(instruction(&sends[0])[0], 5, "a Return");
    let told = calls(&st.fake, "chat_module.send_message", 1).await;
    match sent(&told[0]).body {
        Body::Failed { reason } => reason,
        other => panic!("not a decline: {other:?}"),
    }
}

/// Text that fits no chat message, so it has to go through storage.
fn too_big_for_chat() -> &'static str {
    Box::leak(format!(r#"{{"output": {{"text": "{}"}}}}"#, "x".repeat(70_000)).into_boxed_str())
}

#[tokio::test]
async fn a_cid_input_reaches_the_handler_with_its_content() {
    let (url, seen) = handler(vec![("200 OK", r#"{"output": {"text": "done"}}"#)]);
    let st = paid_storage_job(&url, Payload::Cid(CID.into()), true).await;
    st.fake.set(
        "storage_module.downloadToUrl",
        storage("downloadToUrl", &format!("{CID:?}")),
    );
    st.s.follow_through().await;
    serve_download(&st, 1, b"a long document").await;
    assert_eq!(delivered(&st).await, json!({ "text": "done" }));
    let asked: Value = serde_json::from_str(&seen.lock().unwrap()[0]).unwrap();
    assert_eq!(
        asked["input"],
        json!({ "cid": CID, "content_b64": STANDARD.encode(b"a long document") })
    );
    assert!(left_behind(&st).is_empty(), "{:?}", left_behind(&st));
}

#[tokio::test]
async fn an_input_storage_cant_fetch_for_now_is_fetched_again() {
    let (url, seen) = handler(vec![("200 OK", r#"{"output": {"text": "done"}}"#)]);
    let st = paid_storage_job(&url, Payload::Cid(CID.into()), true).await;
    st.fake.set(
        "storage_module.downloadToUrl",
        storage("downloadToUrl", &format!("{CID:?}")),
    );
    st.s.follow_through().await;
    calls(&st.fake, "storage_module.downloadToUrl", 1).await;
    emit(
        &st.events,
        "storageDownloadDone",
        json!({ "sessionId": CID, "success": false, "error": "no peers have it" }),
    );
    serve_download(&st, 2, b"found it").await;
    assert_eq!(delivered(&st).await, json!({ "text": "done" }));
    assert_eq!(seen.lock().unwrap().len(), 1);
}

#[tokio::test]
async fn an_input_over_the_cap_is_declined_without_the_handler() {
    let (url, seen) = handler(vec![("200 OK", r#"{"output": {"text": "done"}}"#)]);
    let st = paid_storage_job(&url, Payload::Cid(CID.into()), true).await;
    st.fake.set(
        "storage_module.downloadToUrl",
        storage("downloadToUrl", &format!("{CID:?}")),
    );
    st.s.follow_through().await;
    calls(&st.fake, "storage_module.downloadToUrl", 1).await;
    let total = crate::provider::MAX_CONTENT_BYTES + 1;
    emit(
        &st.events,
        "storageDownloadProgress",
        json!({ "sessionId": CID, "bytes": 0, "total": total }),
    );
    let reason = declined(&st).await;
    assert!(reason.contains("MiB"), "{reason}");
    assert!(seen.lock().unwrap().is_empty());
    assert_eq!(
        st.fake.calls_to("storage_module.downloadToUrl").len(),
        1,
        "not fetched again"
    );
}

#[tokio::test]
async fn without_a_files_root_the_handler_gets_the_cid_alone() {
    let (url, seen) = handler(vec![("200 OK", r#"{"output": {"text": "done"}}"#)]);
    let st = paid_storage_job(&url, Payload::Cid(CID.into()), false).await;
    st.s.follow_through().await;
    assert_eq!(delivered(&st).await, json!({ "text": "done" }));
    let asked: Value = serde_json::from_str(&seen.lock().unwrap()[0]).unwrap();
    assert_eq!(asked["input"], json!({ "cid": CID }));
    assert!(st.fake.calls_to("storage_module.downloadToUrl").is_empty());
}

#[tokio::test]
async fn a_content_output_is_uploaded_and_delivered_as_its_cid() {
    let (url, _seen) = handler(vec![(
        "200 OK",
        r#"{"output": {"content_b64": "aGVsbG8="}}"#,
    )]);
    let st = paid_storage_job(&url, Payload::Text("hi".into()), true).await;
    st.fake
        .set("storage_module.uploadUrl", storage("uploadUrl", r#""81""#));
    st.s.follow_through().await;
    let uploaded = serve_upload(&st, 1, "81", json!({ "success": true, "cid": OUT_CID })).await;
    assert_eq!(uploaded, b"hello");
    assert_eq!(delivered(&st).await, json!({ "cid": OUT_CID }));
    assert!(left_behind(&st).is_empty(), "{:?}", left_behind(&st));
}

#[tokio::test]
async fn text_too_big_for_a_chat_message_is_delivered_through_storage() {
    let (url, _seen) = handler(vec![("200 OK", too_big_for_chat())]);
    let st = paid_storage_job(&url, Payload::Text("hi".into()), true).await;
    st.fake
        .set("storage_module.uploadUrl", storage("uploadUrl", r#""82""#));
    st.s.follow_through().await;
    let uploaded = serve_upload(&st, 1, "82", json!({ "success": true, "cid": OUT_CID })).await;
    assert_eq!(uploaded, "x".repeat(70_000).as_bytes());
    assert_eq!(delivered(&st).await, json!({ "cid": OUT_CID }));
}

#[tokio::test]
async fn a_failed_upload_is_tried_again_without_asking_the_handler() {
    let (url, seen) = handler(vec![(
        "200 OK",
        r#"{"output": {"content_b64": "aGVsbG8="}}"#,
    )]);
    let st = paid_storage_job(&url, Payload::Text("hi".into()), true).await;
    st.fake
        .set("storage_module.uploadUrl", storage("uploadUrl", r#""83""#));
    st.fake
        .set("storage_module.uploadUrl", storage("uploadUrl", r#""84""#));
    st.s.follow_through().await;
    serve_upload(
        &st,
        1,
        "83",
        json!({ "success": false, "error": "disk full" }),
    )
    .await;
    let uploaded = serve_upload(&st, 2, "84", json!({ "success": true, "cid": OUT_CID })).await;
    assert_eq!(uploaded, b"hello");
    assert_eq!(delivered(&st).await, json!({ "cid": OUT_CID }));
    assert_eq!(seen.lock().unwrap().len(), 1);
}

#[tokio::test]
async fn without_a_files_root_output_for_storage_is_declined() {
    for answer in [
        r#"{"output": {"content_b64": "aGVsbG8="}}"#,
        too_big_for_chat(),
    ] {
        let (url, _seen) = handler(vec![("200 OK", answer)]);
        let st = paid_storage_job(&url, Payload::Text("hi".into()), false).await;
        st.s.follow_through().await;
        let reason = declined(&st).await;
        assert!(reason.contains("files root"), "{reason}");
    }
}

#[tokio::test]
async fn a_cid_storage_wont_take_is_declined_without_fetching() {
    let (url, seen) = handler(vec![("200 OK", r#"{"output": {"text": "done"}}"#)]);
    let st = paid_storage_job(&url, Payload::Cid("x".repeat(129)), true).await;
    st.s.follow_through().await;
    let reason = declined(&st).await;
    assert!(reason.contains("CID"), "{reason}");
    assert!(seen.lock().unwrap().is_empty());
    assert!(st.fake.calls_to("storage_module.downloadToUrl").is_empty());
}

#[tokio::test]
async fn an_input_found_over_the_cap_once_fetched_is_declined() {
    let (url, seen) = handler(vec![("200 OK", r#"{"output": {"text": "done"}}"#)]);
    let st = paid_storage_job(&url, Payload::Cid(CID.into()), true).await;
    st.fake.set(
        "storage_module.downloadToUrl",
        storage("downloadToUrl", &format!("{CID:?}")),
    );
    st.s.follow_through().await;
    // No progress events: the size shows only once the file is there.
    let args = calls(&st.fake, "storage_module.downloadToUrl", 1).await[0].clone();
    std::fs::write(
        path_in(&args),
        vec![0u8; crate::provider::MAX_CONTENT_BYTES + 1],
    )
    .unwrap();
    emit(
        &st.events,
        "storageDownloadDone",
        json!({ "sessionId": CID, "success": true }),
    );
    let reason = declined(&st).await;
    assert!(reason.contains("MiB"), "{reason}");
    assert!(seen.lock().unwrap().is_empty());
    assert!(left_behind(&st).is_empty(), "{:?}", left_behind(&st));
}

#[tokio::test]
async fn with_storage_switched_off_the_handler_gets_the_cid_alone() {
    let (url, seen) = handler(vec![("200 OK", r#"{"output": {"text": "done"}}"#)]);
    let mut st = paid_storage_job(&url, Payload::Cid(CID.into()), true).await;
    let (groups, rx) = tokio::sync::watch::channel(ToolGroups::default());
    groups.send_modify(|g| g.set("storage", false).unwrap());
    st.s = st.s.with_tool_groups(rx);
    st.s.follow_through().await;
    assert_eq!(delivered(&st).await, json!({ "text": "done" }));
    let asked: Value = serde_json::from_str(&seen.lock().unwrap()[0]).unwrap();
    assert_eq!(asked["input"], json!({ "cid": CID }));
    assert!(st.fake.calls_to("storage_module.downloadToUrl").is_empty());
}

#[tokio::test]
async fn nothing_is_uploaded_once_services_went_off() {
    let (open, gate) = std::sync::mpsc::channel();
    let (url, seen) = handler_gated(
        vec![("200 OK", r#"{"output": {"content_b64": "aGVsbG8="}}"#)],
        Some(gate),
    );
    let mut st = paid_storage_job(&url, Payload::Text("hi".into()), true).await;
    let (groups, rx) = tokio::sync::watch::channel(ToolGroups::default());
    st.s = st.s.with_tool_groups(rx);
    st.s.follow_through().await;
    for _ in 0..200 {
        if !seen.lock().unwrap().is_empty() {
            break;
        }
        tokio::time::sleep(std::time::Duration::from_millis(10)).await;
    }
    // The user turns services off while the handler works.
    groups.send_modify(|g| g.set("services", false).unwrap());
    open.send(()).unwrap();
    tokio::time::sleep(std::time::Duration::from_millis(300)).await;
    assert!(st.fake.calls_to("storage_module.uploadUrl").is_empty());
    assert!(
        st.fake
            .calls_to("lez_core.send_generic_public_transaction")
            .is_empty()
    );
}

#[test]
fn job_files_stay_in_the_files_root() {
    use super::super::jobs::job_file;
    let files = tempfile::tempdir().unwrap();
    let root = FilesRoot::new(files.path()).unwrap();
    assert!(job_file(&root, "../escape", "in").is_err());
    assert!(job_file(&root, "", "in").is_err());
    // Each run gets its own file.
    let (a, full) = job_file(&root, JOB, "out").unwrap();
    let (b, _) = job_file(&root, JOB, "out").unwrap();
    assert_ne!(a, b);
    assert!(full.starts_with(root.path().join("logos-agents-jobs")));
    // The job folder must not lead out of the root.
    let outside = tempfile::tempdir().unwrap();
    let other = tempfile::tempdir().unwrap();
    std::os::unix::fs::symlink(outside.path(), other.path().join("logos-agents-jobs")).unwrap();
    let linked = FilesRoot::new(other.path()).unwrap();
    assert!(job_file(&linked, JOB, "in").is_err());
}

#[test]
fn files_a_stopped_run_left_behind_are_removed_once_stale() {
    use super::super::jobs::job_file;
    let files = tempfile::tempdir().unwrap();
    let root = FilesRoot::new(files.path()).unwrap();
    let (_, recent) = job_file(&root, JOB, "in").unwrap();
    std::fs::write(&recent, b"in use").unwrap();
    let (_, stale) = job_file(&root, JOB, "out").unwrap();
    std::fs::write(&stale, b"left behind").unwrap();
    let three_days = std::time::Duration::from_secs(3 * 24 * 60 * 60);
    std::fs::File::options()
        .write(true)
        .open(&stale)
        .unwrap()
        .set_modified(std::time::SystemTime::now() - three_days)
        .unwrap();
    job_file(&root, JOB, "in").unwrap();
    assert!(recent.exists());
    assert!(!stale.exists());
}
