use super::*;
use std::sync::Mutex;
use tokio::io::{AsyncBufReadExt, AsyncWriteExt, BufReader};

#[derive(Debug)]
struct Request {
    head: String,
    body: Vec<u8>,
}
struct Reply {
    status: u16,
    headers: String,
    body: String,
}
fn reply(status: u16, body: Value) -> Reply {
    Reply {
        status,
        headers: String::new(),
        body: body.to_string(),
    }
}
async fn mock(
    kind: CloudProvider,
    responses: Vec<Reply>,
) -> (
    HttpStorage,
    Arc<Mutex<Vec<Request>>>,
    tokio::task::JoinHandle<()>,
) {
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let base = format!("http://{}", listener.local_addr().unwrap());
    let replacement = base.clone();
    let recorded = Arc::new(Mutex::new(Vec::new()));
    let requests = recorded.clone();
    let task = tokio::spawn(async move {
        for response in responses {
            let (stream, _) =
                tokio::time::timeout(std::time::Duration::from_secs(10), listener.accept())
                    .await
                    .unwrap()
                    .unwrap();
            let mut reader = BufReader::new(stream);
            let mut head = String::new();
            let mut size = 0;
            loop {
                let mut line = String::new();
                reader.read_line(&mut line).await.unwrap();
                if line == "\r\n" {
                    break;
                }
                if let Some((key, value)) = line.split_once(':')
                    && key.eq_ignore_ascii_case("content-length")
                {
                    size = value.trim().parse::<usize>().unwrap();
                }
                head.push_str(&line);
            }
            let mut body = vec![0; size];
            reader.read_exact(&mut body).await.unwrap();
            requests.lock().unwrap().push(Request { head, body });
            let body = response.body.replace("$BASE", &replacement);
            let headers = response.headers.replace("$BASE", &replacement);
            let output = format!(
                "HTTP/1.1 {} Mock\r\nConnection: close\r\nContent-Type: application/json\r\nContent-Length: {}\r\n{}\r\n{}",
                response.status,
                body.len(),
                headers,
                body
            );
            reader.get_mut().write_all(output.as_bytes()).await.unwrap();
            reader.get_mut().shutdown().await.unwrap();
        }
    });
    let mut provider = HttpStorage::production(kind, "TEST-TOKEN-NEVER-LOG".into()).unwrap();
    provider.api = base.clone();
    provider.content = base;
    provider.testing = true;
    (provider, recorded, task)
}
async fn file(size: usize) -> (tempfile::TempDir, CloudFile) {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("relatório.txt");
    let bytes = vec![b'A'; size];
    tokio::fs::write(&path, &bytes).await.unwrap();
    let file = CloudFile {
        path,
        name: "relatório.txt".into(),
        size: size as u64,
        sha256: format!("{:x}", Sha256::digest(&bytes)),
    };
    (dir, file)
}
fn uploaded() -> UploadedFile {
    UploadedFile {
        id: "id:file".into(),
        name: "test.txt".into(),
        size: 10,
    }
}
#[tokio::test]
async fn drive_chunked_upload_and_reader_link() {
    let (_dir, file) = file(CHUNK + 17).await;
    let responses = vec![
        Reply {
            status: 200,
            headers: "Location: $BASE/session\r\n".into(),
            body: "{}".into(),
        },
        Reply {
            status: 308,
            headers: format!("Range: bytes=0-{}\r\n", CHUNK - 1),
            body: "".into(),
        },
        reply(200, json!({"id":"drive-file"})),
        reply(200, json!({"id":"anyone"})),
    ];
    let (provider, requests, task) = mock(CloudProvider::GoogleDrive, responses).await;
    let progress = Arc::new(Mutex::new(Vec::new()));
    let seen = progress.clone();
    let uploaded = provider
        .upload(
            &file,
            Arc::new(move |done, _| seen.lock().unwrap().push(done)),
        )
        .await
        .unwrap();
    let link = provider.share(&uploaded, None).await.unwrap();
    assert!(link.url.contains("drive-file/view"));
    task.await.unwrap();
    let requests = requests.lock().unwrap();
    assert!(
        requests[0]
            .head
            .starts_with("POST /files?uploadType=resumable")
    );
    assert_eq!(requests[1].body.len(), CHUNK);
    assert_eq!(requests[2].body.len(), 17);
    assert!(
        requests[1]
            .head
            .to_ascii_lowercase()
            .contains("authorization: bearer test-token-never-log")
    );
    let permission: Value = serde_json::from_slice(&requests[3].body).unwrap();
    assert_eq!(permission["type"], "anyone");
    assert_eq!(permission["role"], "reader");
    assert_eq!(permission["allowFileDiscovery"], false);
    assert_eq!(*progress.lock().unwrap(), [CHUNK as u64, file.size]);
}
#[tokio::test]
async fn onedrive_chunks_never_leak_graph_bearer_and_confirm_expiry() {
    let (_dir, file) = file(CHUNK + 11).await;
    let (provider,requests,task)=mock(CloudProvider::OneDrive,vec![reply(200,json!({"uploadUrl":"$BASE/upload?secret=presigned"})),reply(202,json!({"nextExpectedRanges":[format!("{CHUNK}-")]})),reply(201,json!({"id":"one-file"})),reply(201,json!({"link":{"scope":"anonymous","type":"view","webUrl":"https://1drv.ms/s/synthetic"},"expirationDateTime":"2099-01-01T00:00:00.000Z"}))]).await;
    let uploaded = provider.upload(&file, Arc::new(|_, _| {})).await.unwrap();
    let link = provider
        .share(&uploaded, Some("2099-01-01T00:00:00+00:00"))
        .await
        .unwrap();
    assert!(link.expires_at.is_some());
    task.await.unwrap();
    let requests = requests.lock().unwrap();
    assert!(
        requests[0]
            .head
            .to_ascii_lowercase()
            .contains("authorization: bearer")
    );
    for request in &requests[1..3] {
        assert!(!request.head.to_ascii_lowercase().contains("authorization:"));
        assert!(request.head.contains("secret=presigned"));
    }
    assert_eq!(requests[1].body.len(), CHUNK);
    let sharing: Value = serde_json::from_slice(&requests[3].body).unwrap();
    assert_eq!(sharing["scope"], "anonymous");
    assert_eq!(sharing["type"], "view");
}
#[tokio::test]
async fn dropbox_session_offsets_unicode_header_and_public_viewer() {
    let (_dir, file) = file(CHUNK + 3).await;
    let (provider,requests,task)=mock(CloudProvider::Dropbox,vec![reply(200,json!({"session_id":"session"})),reply(200,Value::Null),reply(200,json!({"id":"id:dropbox"})),reply(200,json!({"url":"https://www.dropbox.com/scl/fi/mock/report?dl=0","link_permissions":{"resolved_visibility":{".tag":"public"},"link_access_level":{".tag":"viewer"}},"expires":"2099-01-01T00:00:00Z"}))]).await;
    let uploaded = provider.upload(&file, Arc::new(|_, _| {})).await.unwrap();
    provider
        .share(&uploaded, Some("2099-01-01T00:00:00+00:00"))
        .await
        .unwrap();
    task.await.unwrap();
    let requests = requests.lock().unwrap();
    assert!(requests[0].body.is_empty());
    assert!(
        requests[1]
            .head
            .starts_with("POST /files/upload_session/append_v2")
    );
    assert_eq!(requests[1].body.len(), CHUNK);
    let arg = requests[2]
        .head
        .lines()
        .find_map(|s| {
            s.split_once(':')
                .filter(|(key, _)| key.eq_ignore_ascii_case("dropbox-api-arg"))
                .map(|(_, v)| v.trim())
        })
        .unwrap();
    assert!(arg.is_ascii());
    let arg: Value = serde_json::from_str(arg).unwrap();
    assert_eq!(arg["cursor"]["offset"], CHUNK);
    assert!(
        arg["commit"]["path"]
            .as_str()
            .unwrap()
            .ends_with("relatório.txt")
    );
    assert_eq!(arg["commit"]["mode"], "add");
    let settings: Value = serde_json::from_slice(&requests[3].body).unwrap();
    assert_eq!(settings["settings"]["access"], "viewer");
    assert_eq!(settings["settings"]["audience"], "public");
}
#[tokio::test]
async fn business_policy_does_not_silently_downgrade_to_organization_or_edit() {
    for (kind, response) in [
        (
            CloudProvider::OneDrive,
            json!({"link":{"scope":"organization","type":"edit","webUrl":"https://example.invalid"}}),
        ),
        (
            CloudProvider::Dropbox,
            json!({"url":"https://example.invalid","link_permissions":{"resolved_visibility":{".tag":"team_only"}}}),
        ),
    ] {
        let (provider, _, task) = mock(kind, vec![reply(200, response)]).await;
        assert!(provider.share(&uploaded(), None).await.is_err());
        task.await.unwrap();
    }
}
#[tokio::test]
async fn missing_expiry_and_provider_error_are_safe() {
    let (provider,_,task)=mock(CloudProvider::Dropbox,vec![reply(200,json!({"url":"https://example.invalid","link_permissions":{"resolved_visibility":{".tag":"public"}}}))]).await;
    assert!(
        provider
            .share(&uploaded(), Some("2099-01-01T00:00:00Z"))
            .await
            .unwrap_err()
            .to_string()
            .contains("expiry")
    );
    task.await.unwrap();
    let (provider, _, task) = mock(
        CloudProvider::OneDrive,
        vec![reply(403, json!({"secret":"MUST_NOT_BE_LOGGED"}))],
    )
    .await;
    let error = provider
        .share(&uploaded(), None)
        .await
        .unwrap_err()
        .to_string();
    assert!(!error.contains("MUST_NOT_BE_LOGGED"));
    assert!(!error.contains("TEST-TOKEN"));
    task.await.unwrap();
}
#[tokio::test]
async fn changed_snapshot_never_opens_upload_session() {
    let (_dir, file) = file(4).await;
    tokio::fs::write(&file.path, b"evil").await.unwrap();
    let provider = HttpStorage::production(CloudProvider::GoogleDrive, "fake".into()).unwrap();
    assert!(
        provider
            .upload(&file, Arc::new(|_, _| panic!("not uploaded")))
            .await
            .is_err()
    );
}
#[test]
fn session_origin_allowlist_and_expiry_formats() {
    let provider = HttpStorage::production(CloudProvider::GoogleDrive, "fake".into()).unwrap();
    for url in [
        "http://www.googleapis.com/upload",
        "https://www.googleapis.com.attacker.invalid/x",
        "https://user:secret@www.googleapis.com/x",
        "file:///etc/passwd",
    ] {
        assert!(provider.session_url(url).is_err());
    }
    assert!(
        provider
            .session_url("https://www.googleapis.com/upload")
            .is_ok()
    );
    assert!(expiry_matches(
        Some("2099-01-01T00:00:00Z"),
        Some("2098-12-31T20:00:00-04:00")
    ));
    assert!(!expiry_matches(Some("2099-01-01T00:00:00Z"), None));
}
#[tokio::test]
async fn cleanup_uses_new_file_id() {
    for kind in [
        CloudProvider::GoogleDrive,
        CloudProvider::OneDrive,
        CloudProvider::Dropbox,
    ] {
        let (provider, requests, task) = mock(kind, vec![reply(200, json!({}))]).await;
        provider.delete("new-id").await.unwrap();
        task.await.unwrap();
        let requests = requests.lock().unwrap();
        if kind == CloudProvider::Dropbox {
            assert_eq!(
                serde_json::from_slice::<Value>(&requests[0].body).unwrap()["path"],
                "new-id"
            );
        } else {
            assert!(requests[0].head.starts_with("DELETE "));
            assert!(requests[0].head.contains("new-id"));
        }
    }
}
