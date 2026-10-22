use hyper::StatusCode;
use lanflare_core::server::dufs::{
    parse_range_header, resolve_and_validate_path, start_standalone_dufs_server, DufsConfig,
};

#[test]
fn test_range_header_parsing() {
    assert_eq!(parse_range_header("bytes=0-499", 1000), Some((0, 499)));
    assert_eq!(parse_range_header("bytes=500-999", 1000), Some((500, 999)));
    assert_eq!(parse_range_header("bytes=500-", 1000), Some((500, 999)));
    assert_eq!(parse_range_header("bytes=-100", 1000), Some((900, 999)));
    assert_eq!(parse_range_header("bytes=1500-", 1000), None);
    assert_eq!(parse_range_header("bytes=600-500", 1000), None);
    assert_eq!(parse_range_header("invalid", 1000), None);
}

#[test]
fn test_path_traversal_guard() {
    let temp_root = std::env::temp_dir().join(format!("dufs_test_path_{}", uuid::Uuid::new_v4()));
    std::fs::create_dir_all(&temp_root).unwrap();

    let safe_file = temp_root.join("hello.txt");
    std::fs::write(&safe_file, b"world").unwrap();

    // Normal access
    let res = resolve_and_validate_path(&temp_root, "/hello.txt", "");
    assert!(res.is_ok());

    // Percent-encoded normal access
    let res = resolve_and_validate_path(&temp_root, "/%68ello.txt", "");
    assert!(res.is_ok());

    // Traversal attempts
    let res = resolve_and_validate_path(&temp_root, "/../secret.txt", "");
    assert!(res.is_err());
    assert_eq!(res.unwrap_err(), StatusCode::FORBIDDEN);

    let res = resolve_and_validate_path(&temp_root, "/%2e%2e/secret.txt", "");
    assert!(res.is_err());
    assert_eq!(res.unwrap_err(), StatusCode::FORBIDDEN);

    let res = resolve_and_validate_path(&temp_root, "/sub/../../secret.txt", "");
    assert!(res.is_err());

    let _ = std::fs::remove_dir_all(&temp_root);
}

#[tokio::test]
async fn test_dufs_standalone_server_e2e() {
    let temp_root = std::env::temp_dir().join(format!("dufs_e2e_{}", uuid::Uuid::new_v4()));
    std::fs::create_dir_all(&temp_root).unwrap();

    let sub_dir = temp_root.join("documents");
    std::fs::create_dir_all(&sub_dir).unwrap();

    let file1 = temp_root.join("hello.txt");
    std::fs::write(&file1, b"Hello, LanFlare and Dufs!").unwrap();

    let file2 = sub_dir.join("notes.txt");
    std::fs::write(&file2, b"Important meeting notes").unwrap();

    let config = DufsConfig {
        root_path: temp_root.clone(),
        path_prefix: String::new(),
        allow_upload: true,
        allow_delete: true,
        auth_pin: Some("secret123".to_string()),
        title: "Test Share".to_string(),
    };

    // Bind to port 0 for an ephemeral port
    let handle = start_standalone_dufs_server(config, 0).await.unwrap();
    let port = handle.port;
    assert!(port > 0);

    let client = reqwest::Client::new();
    let base_url = format!("http://127.0.0.1:{port}");

    // 1. Without PIN -> 401 Unauthorized with login hint in WWW-Authenticate
    let res = client.get(&base_url).send().await.unwrap();
    assert_eq!(res.status(), reqwest::StatusCode::UNAUTHORIZED);
    let auth_header = res.headers()["www-authenticate"].to_str().unwrap();
    assert!(auth_header.contains("Enter Password (Leave Username Blank / Not Required)"));

    // 2. With PIN via query param -> 200 OK (HTML directory listing)
    let res = client
        .get(format!("{base_url}/?pin=secret123"))
        .send()
        .await
        .unwrap();
    assert_eq!(res.status(), reqwest::StatusCode::OK);
    let html = res.text().await.unwrap();
    assert!(html.contains("Test Share"));
    assert!(html.contains("hello.txt"));
    assert!(html.contains("documents"));

    // 3. With PIN via Basic Auth in password field -> 200 OK
    let res = client
        .get(format!("{base_url}/"))
        .basic_auth("", Some("secret123"))
        .send()
        .await
        .unwrap();
    assert_eq!(res.status(), reqwest::StatusCode::OK);

    // 3b. With PIN via Basic Auth in username field (leaving password empty) -> 200 OK
    let res = client
        .get(format!("{base_url}/"))
        .basic_auth("secret123", Option::<&str>::None)
        .send()
        .await
        .unwrap();
    assert_eq!(res.status(), reqwest::StatusCode::OK);

    // 4. JSON directory listing (?json)
    let res = client
        .get(format!("{base_url}/?json&pin=secret123"))
        .send()
        .await
        .unwrap();
    assert_eq!(res.status(), reqwest::StatusCode::OK);
    let json_val: serde_json::Value = res.json().await.unwrap();
    assert!(json_val.is_array());
    let entries = json_val.as_array().unwrap();
    assert!(entries.iter().any(|e| e["name"] == "hello.txt" && e["is_dir"] == false));
    assert!(entries.iter().any(|e| e["name"] == "documents" && e["is_dir"] == true));

    // 5. File streaming with Range request
    let res = client
        .get(format!("{base_url}/hello.txt?pin=secret123"))
        .header("Range", "bytes=0-4")
        .send()
        .await
        .unwrap();
    assert_eq!(res.status(), reqwest::StatusCode::PARTIAL_CONTENT);
    let bytes = res.bytes().await.unwrap();
    assert_eq!(&bytes[..], b"Hello");

    // 6. Subdirectory browsing
    let res = client
        .get(format!("{base_url}/documents/?pin=secret123"))
        .send()
        .await
        .unwrap();
    assert_eq!(res.status(), reqwest::StatusCode::OK);
    let html = res.text().await.unwrap();
    assert!(html.contains("notes.txt"));

    // 7. Directory ZIP download (?zip)
    let res = client
        .get(format!("{base_url}/?zip&pin=secret123"))
        .send()
        .await
        .unwrap();
    assert_eq!(res.status(), reqwest::StatusCode::OK);
    assert_eq!(res.headers()["content-type"], "application/zip");
    let zip_bytes = res.bytes().await.unwrap();
    assert!(!zip_bytes.is_empty());

    // Validate the zip archive contents using zip crate
    let reader = std::io::Cursor::new(zip_bytes);
    let mut zip_archive = zip::ZipArchive::new(reader).unwrap();
    let mut names = Vec::new();
    for i in 0..zip_archive.len() {
        let file = zip_archive.by_index(i).unwrap();
        names.push(file.name().to_string());
    }
    assert!(names.iter().any(|n| n == "hello.txt"));
    assert!(names.iter().any(|n| n == "documents/notes.txt" || n == "documents/"));

    // 8. Upload file via PUT
    let upload_content = b"Uploaded by remote peer over localnet";
    let res = client
        .put(format!("{base_url}/uploaded.txt?upload&pin=secret123"))
        .body(upload_content.to_vec())
        .send()
        .await
        .unwrap();
    assert_eq!(res.status(), reqwest::StatusCode::CREATED);
    assert_eq!(
        std::fs::read(temp_root.join("uploaded.txt")).unwrap(),
        upload_content
    );

    // Stop server and clean up
    handle.stop();
    let _ = std::fs::remove_dir_all(&temp_root);
}

#[test]
fn test_stronger_password_generation() {
    for _ in 0..50 {
        let pin = lanflare_core::config::generate_share_link_pin();
        assert_eq!(pin.len(), 10, "Default password should be 10 characters long");
        assert!(
            pin.chars().all(|c| c.is_ascii_alphanumeric() && c != '@' && c != ':' && c != '/'),
            "Password must contain only safe alphanumeric characters"
        );
    }
}

#[tokio::test]
async fn test_http_to_https_auto_redirect() {
    use lanflare_core::config::AppConfig;
    use lanflare_core::crypto::cert::generate_self_signed;
    use lanflare_core::discovery::registry::PeerRegistry;
    use lanflare_core::server::start_server;
    use tokio::io::{AsyncReadExt, AsyncWriteExt};
    use tokio::net::TcpStream;

    let cert_ident = generate_self_signed().unwrap();
    let config = AppConfig {
        port: 0,
        ..Default::default()
    };
    let registry = PeerRegistry::new();

    // Start HTTPS server
    let server_handle = start_server(config, Some(cert_ident), registry, None)
        .await
        .expect("HTTPS server should start");
    let port = server_handle.bound_port();

    // Send a plaintext HTTP request to the HTTPS port
    let mut stream = TcpStream::connect(format!("127.0.0.1:{port}")).await.unwrap();
    let http_request = format!("GET /dufs/test?pin=foo HTTP/1.1\r\nHost: 127.0.0.1:{port}\r\nConnection: close\r\n\r\n");
    stream.write_all(http_request.as_bytes()).await.unwrap();

    let mut response_buf = Vec::new();
    stream.read_to_end(&mut response_buf).await.unwrap();
    let response_str = String::from_utf8_lossy(&response_buf);

    // Verify redirect response
    assert!(
        response_str.starts_with("HTTP/1.1 301 Moved Permanently"),
        "Plaintext HTTP should be redirected with 301 Moved Permanently, got: {}",
        response_str
    );
    assert!(
        response_str.to_lowercase().contains(&format!("location: https://127.0.0.1:{port}/dufs/test?pin=foo")),
        "Response should include Location header pointing to https://, got: {}",
        response_str
    );
}
