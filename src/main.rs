use std::{collections::HashMap, net::SocketAddr, sync::Arc};
use axum::{
    routing::{get, post},
    Router,
    http::StatusCode,
    extract::{State, Json}
};
use clap::Parser;
use reqwest::Client;
use serde::{Serialize, Deserialize};
use tokio::sync::Mutex;

#[derive(Parser, Debug)]
#[command(author, version, about, long_about = None)]
struct Args {
    #[arg(long)]
    id: u64,
    #[arg(short, long)]
    port: String,
}

type Id = u64;
type Value = String;


#[derive(Clone, Debug, Serialize, Deserialize)]
struct NodeWithLedger {
    pub id: u64,
    pub addr: SocketAddr,
    pub ledger: HashMap<Id, Value>,
}

impl NodeWithLedger {
    pub fn new(id: u64, addr: SocketAddr) -> Self{
        Self {
            id,
            addr,
            ledger: HashMap::new(),
        }
    }
}

#[derive(Clone, Debug, Serialize, Deserialize)]
struct Node {
    pub id: u64,
    pub addr: SocketAddr,
}

#[derive(Clone, Debug)]
struct AppState {
    node: NodeWithLedger,
    nodes: Arc<Mutex<Vec<Node>>>,
}

#[tokio::main]
async fn main() {
    let args = Args::parse();
    let port = args.port;
    let node_id = args.id;

    let node_http_addr = format!("0.0.0.0:{}", port);

    println!("Starting new node: http://{}", node_http_addr);

    let node = NodeWithLedger::new(node_id, node_http_addr.parse().unwrap());
    let state = AppState {
        node,
        nodes: Arc::new(Mutex::new(Vec::new())),
    };

    let app = Router::new()
        .route("/", get(get_node_state))
        .route("/ping", post(ping))
        .route("/connect", post(connect))
        .route("/propose", post(prepare_proposal))
        .with_state(state);

    let listener = tokio::net::TcpListener::bind(node_http_addr).await.unwrap();
    axum::serve(listener, app).await.unwrap();
}

async fn connect(State(state): State<AppState>, value: String) -> (StatusCode, String) {
    let NodeWithLedger { id, addr, ledger: _ledger } = state.node;

    let mut payload = HashMap::new();
    payload.insert("id", id.to_string());
    payload.insert("addr", addr.to_string());

    let client = Client::new();
    let res = client.post(format!("http://0.0.0.0:{}/ping", value))
        .json(&payload)
        .send()
        .await;

    match res {
        Err(_) => todo!(),
        Ok(res) => {
            let is_error = res.status().is_client_error() || res.status().is_server_error();
            if is_error {
                return (StatusCode::BAD_REQUEST, res.text().await.unwrap());
            }

            let body_text = res.text().await.unwrap();
            let body: PingNode = serde_json::from_str(body_text.as_str()).unwrap();

            let mut nodes = state.nodes.lock().await;

            let id: u64 = body.id.parse().unwrap();
            let addr: SocketAddr = body.addr.parse().unwrap();
            nodes.push(Node { id, addr });

            println!("[/connect] sync new node: {} - ID: {}", addr, id);

            (StatusCode::OK, format!("Conneted to new voter: {}!", value))
        }
    }
}

#[derive(Deserialize, Debug)]
struct PingNode {
    pub id: String,
    pub addr: String,
}

async fn ping(
    State(state): State<AppState>,
    Json(body): Json<PingNode>
) -> (StatusCode, Json<HashMap<&'static str, String>>) {
    let node_id: u64 = body.id.parse().unwrap();
    if node_id == state.node.id {
        let mut payload = HashMap::new();
        payload.insert("error", String::from("You can't connect in the same node!"));
        return (StatusCode::BAD_REQUEST, Json(payload));
    }

    let mut nodes = state.nodes.lock().await;

    if nodes.iter().any(|node| node.id == node_id) {
        let mut payload = HashMap::new();
        payload.insert("error", String::from("You're already connected in this node!"));
        return (StatusCode::BAD_REQUEST, Json(payload));
    }

    nodes.push(Node { id: node_id, addr: body.addr.parse().unwrap() });

    println!("[/ping] updated state: {:?}", state);

    let mut payload = HashMap::new();
    payload.insert("id", state.node.id.to_string());
    payload.insert("addr", state.node.addr.to_string());

    (StatusCode::OK, Json(payload))
}

async fn get_node_state(State(state): State<AppState>) -> (StatusCode, String) {
    println!("[/] State: {:?}", state);
    let state = serde_json::to_string(&state.node).unwrap();
    (StatusCode::OK, state)
}

async fn prepare_proposal() -> (StatusCode, ()) {
    todo!()
}
