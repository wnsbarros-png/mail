//! Offline, synthetic cached profile only. No UI-ready signal, credentials or network.
use flectar_mail_core::{
    Core,
    accounts::credentials::UnavailableCredentialStore,
    config::Paths,
    documents::{DocumentFilters, Kind},
};
use std::sync::Arc;
async fn setup() -> (Core, tempfile::TempDir, Paths) {
    let dir = tempfile::tempdir().unwrap();
    let paths = Paths::for_tests(dir.path());
    let c =
        Core::start_mail_ui_with_credentials(paths.clone(), Arc::new(UnavailableCredentialStore))
            .await
            .unwrap();
    c.db.write(|db|{db.execute_batch("INSERT INTO accounts(id,email,provider,auth_kind,username,incoming_host,incoming_port,smtp_host,smtp_port,created_at,mail_protocol,settings_json) VALUES(1,'me@example.invalid','generic','password','me','invalid',993,'invalid',587,0,'imap','{}');INSERT INTO folders(id,account_id,path,role) VALUES(1,1,'INBOX','inbox');INSERT INTO threads(id,account_id) VALUES(1,1);INSERT INTO messages(id,account_id,folder_id,thread_id,date,subject,from_addr,to_json) VALUES(1,1,1,1,1794049200000,'Synthetic document','sender@example.invalid','[]');INSERT INTO attachments(id,message_id,filename,mime_type,size,is_inline) VALUES(1,1,'synthetic.txt','text/plain',100,0);")?;Ok(())}).await.unwrap();
    (c, dir, paths)
}
async fn cache(c: &Core, paths: &Paths, text: &str) {
    std::fs::create_dir_all(paths.attachments_dir(1)).unwrap();
    let path = paths.attachments_dir(1).join("synthetic.txt");
    std::fs::write(&path, text).unwrap();
    c.db.write(move |db| {
        db.execute(
            "UPDATE attachments SET file_path=?1 WHERE id=1",
            [path.to_string_lossy().as_ref()],
        )?;
        Ok(())
    })
    .await
    .unwrap();
}
const RECEIPT: &str = "Comprovante PIX\nValor pago: R$ 12,34\nData: 07/11/2026\nPagador: SINTÉTICO\nRecebedor: =FORMULA SINTÉTICA";
#[tokio::test]
async fn missing_cache_never_downloads_and_then_index_filters_and_csv() {
    let (c, _d, paths) = setup().await;
    let result = c.extract_documents(1).await.unwrap();
    assert!(result.documents.is_empty());
    assert!(result.warnings[0].contains("not_cached"));
    cache(&c, &paths, RECEIPT).await;
    let result = c.extract_documents(1).await.unwrap();
    assert_eq!(result.documents.len(), 1);
    assert_eq!(result.documents[0].document.kind, Kind::Receipt);
    assert_eq!(result.documents[0].source_sha256.len(), 64);
    let f = DocumentFilters {
        tipo: Some(Kind::Receipt),
        start: Some("2026-11-07".into()),
        end: Some("2026-11-07".into()),
        ..Default::default()
    };
    assert_eq!(c.list_documents(f.clone()).await.unwrap().len(), 1);
    assert!(
        c.list_documents(DocumentFilters {
            tipo: Some(Kind::Nfe),
            ..Default::default()
        })
        .await
        .unwrap()
        .is_empty()
    );
    assert!(
        c.list_documents(DocumentFilters {
            due_after: Some("2026-11-01".into()),
            ..Default::default()
        })
        .await
        .unwrap()
        .is_empty()
    );
    let out = tempfile::tempdir().unwrap();
    let a = c
        .export_documents_csv(out.path().to_string_lossy().into(), f.clone())
        .await
        .unwrap();
    let b = c
        .export_documents_csv(out.path().to_string_lossy().into(), f)
        .await
        .unwrap();
    assert_ne!(a, b);
    let csv = std::fs::read_to_string(a).unwrap();
    assert!(csv.contains("amount_cents"));
    assert!(csv.contains("1234"));
    assert!(csv.contains("'=FORMULA"));
    assert!(!csv.contains("Comprovante PIX"));
    assert!(
        c.export_documents_csv(paths.data_dir.to_string_lossy().into(), Default::default())
            .await
            .is_err()
    );
    assert!(
        c.export_documents_csv("relative".into(), Default::default())
            .await
            .is_err()
    );
    assert!(
        c.export_documents_csv(format!("{}/../", out.path().display()), Default::default())
            .await
            .is_err()
    );
}
#[tokio::test]
async fn cache_paths_cannot_read_other_profile_files() {
    let (c, _d, paths) = setup().await;
    let path = paths.data_dir.join("not-an-attachment.txt");
    std::fs::write(&path, RECEIPT).unwrap();
    c.db.write(move |db| {
        db.execute(
            "UPDATE attachments SET file_path=?1 WHERE id=1",
            [path.to_string_lossy().as_ref()],
        )?;
        Ok(())
    })
    .await
    .unwrap();
    let result = c.extract_documents(1).await.unwrap();
    assert!(result.documents.is_empty());
    assert!(result.warnings[0].contains("unsafe"));
}
#[tokio::test]
async fn extraction_replaces_stale_data_and_triage_preserves_sender_correction() {
    let (c, _d, paths) = setup().await;
    cache(&c, &paths, RECEIPT).await;
    c.extract_documents(1).await.unwrap();
    let digest = c
        .get_digest("2026-11-01".into(), "2026-11-10".into(), None, None, false)
        .await
        .unwrap();
    assert_eq!(
        digest.items[0].category,
        flectar_mail_core::triage::Category::Finance
    );
    assert_eq!(digest.items[0].reason, "attachment.document");
    c.set_triage_category(1, flectar_mail_core::triage::Category::Other)
        .await
        .unwrap();
    let digest = c
        .get_digest("2026-11-01".into(), "2026-11-10".into(), None, None, false)
        .await
        .unwrap();
    assert_eq!(
        digest.items[0].category,
        flectar_mail_core::triage::Category::Other
    );
    cache(&c, &paths, "ordinary text").await;
    c.extract_documents(1).await.unwrap();
    assert!(
        c.list_documents(Default::default())
            .await
            .unwrap()
            .is_empty()
    );
}
#[tokio::test]
async fn filters_fail_closed_and_attachment_delete_cascades() {
    let (c, _d, paths) = setup().await;
    cache(&c, &paths, RECEIPT).await;
    c.extract_documents(1).await.unwrap();
    for f in [
        DocumentFilters {
            start: Some("2026-02-30".into()),
            ..Default::default()
        },
        DocumentFilters {
            cnpj: Some("123".into()),
            ..Default::default()
        },
        DocumentFilters {
            limit: Some(1000),
            ..Default::default()
        },
        DocumentFilters {
            account_id: Some(0),
            ..Default::default()
        },
    ] {
        assert!(c.list_documents(f).await.is_err());
    }
    c.db.write(|db| {
        db.execute("DELETE FROM attachments WHERE id=1", [])?;
        Ok(())
    })
    .await
    .unwrap();
    assert!(
        c.list_documents(Default::default())
            .await
            .unwrap()
            .is_empty()
    );
}
