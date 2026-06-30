//! M0.2 gate: two iroh nodes connect by id and exchange a ping (direct, no relay).

use arvolo_core::node::Node;

#[tokio::test]
async fn two_nodes_ping_direct() {
    let server = Node::bind_local().await.expect("bind server");
    let client = Node::bind_local().await.expect("bind client");

    let server_addr = server.local_addr();
    let server_id = server.id();

    let server_task = tokio::spawn(async move { server.serve_one_ping().await });

    client.ping(server_addr).await.expect("ping failed");

    let pinged_by = server_task
        .await
        .expect("server task panicked")
        .expect("server failed");
    assert_eq!(pinged_by, client.id(), "server saw a different client id");
    assert_ne!(pinged_by, server_id);
}
