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

#[derive(Clone, Debug, Default)]
struct Acceptor {
    pub last_ballot_number: u64,
    pub accepted_proposal: Option<Propose>,
}

#[derive(Clone, Debug, Serialize, Deserialize)]
struct Node {
    pub id: u64,
    pub addr: SocketAddr,
}

impl Node {
    pub fn new(id: u64, addr: SocketAddr) -> Self{
        Self { id, addr }
    }
}

type Ledger = HashMap<Id, Value>;

#[derive(Clone, Debug)]
struct AppState {
    node: Node,
    nodes: Arc<Mutex<Vec<Node>>>,
    acceptor: Arc<Mutex<Acceptor>>,
    proposer: Arc<Mutex<Proposer>>,
    ledger: Arc<Mutex<Ledger>>,
}

#[tokio::main]
async fn main() {
    let args = Args::parse();
    let port = args.port;
    let node_id = args.id;

    let node_http_addr = format!("0.0.0.0:{}", port);

    println!("Starting new node: http://{}", node_http_addr);

    let node = Node::new(node_id, node_http_addr.parse().unwrap());
    let state = AppState {
        node,
        nodes: Arc::new(Mutex::new(Vec::new())),
        acceptor: Arc::new(Mutex::new(Acceptor::default())),
        proposer: Arc::new(Mutex::new(Proposer::new())),
        ledger: Arc::new(Mutex::new(HashMap::new())),
    };

    let app = Router::new()
        .route("/", get(get_node_state))
        .route("/state", get(get_state))
        .route("/ping", post(ping))
        .route("/connect", post(connect))
        .route("/prepare", post(prepare))
        .route("/handle-prepare", post(handle_prepare))
        .route("/handle-accept", post(handle_accept))
        .route("/handle-learn", post(handle_learn))
        .with_state(state);

    let listener = tokio::net::TcpListener::bind(node_http_addr).await.unwrap();
    axum::serve(listener, app).await.unwrap();
}

async fn connect(State(state): State<AppState>, value: String) -> (StatusCode, String) {
    let Node { id, addr } = state.node;

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

async fn get_state(State(state): State<AppState>) -> (StatusCode, ()) {
    println!("State: {:?}", state);
    (StatusCode::OK, ())
}

async fn prepare(State(state): State<AppState>, value: String) -> (StatusCode, String) {
    let mut proposer = state.proposer.lock().await;
    let ballot = match proposer.prepare(&state, value).await {
        Err(e) => return (StatusCode::BAD_REQUEST, e.clone()),
        Ok(ballot) => ballot,
    };

    match proposer.propose(&state, &ballot).await {
        Err(e) => (StatusCode::BAD_REQUEST, e),
        Ok(_) => {
            let client = Client::new();
            let mut ledger = state.ledger.lock().await;
            let nodes = state.nodes.lock().await;

            let reqs = nodes.iter().map(|node| {
                client.post(format!("http://{}/handle-learn", node.addr))
                    .json(&ballot)
                    .send()
            });

            futures::future::join_all(reqs).await;

            ledger.insert(ballot.id, ballot.value.unwrap_or(String::from("")));

            (StatusCode::OK, String::from("Proposal accepted by the majority!"))
        },
    }
}

#[derive(Serialize, Deserialize, Debug)]
struct HandleProposalPayload {
    error: Option<String>,
    value: Option<Ballot>,
}

async fn handle_prepare(State(state): State<AppState>, proposal_id: String) -> (StatusCode, Json<HandleProposalPayload>) {
    let proposal_id: u64 = proposal_id.parse().unwrap();
    let mut acceptor = state.acceptor.lock().await;

    if proposal_id < acceptor.last_ballot_number {
        let payload = HandleProposalPayload {
            error: Some(String::from("The proposal ID is lesser than the last accepted ballot number")),
            value: None,
        };
        return (StatusCode::BAD_REQUEST, Json(payload));
    }

    acceptor.last_ballot_number = proposal_id;

    println!("[/handle-prepare] setting the new last ballot number as: {}", proposal_id);

    if acceptor.accepted_proposal.is_some() {
        println!("[/handle-prepare] Node {} already has a value: {:?}", state.node.id, acceptor.accepted_proposal);
        let value = acceptor.accepted_proposal.clone();
        let payload = HandleProposalPayload {
            error: None,
            value: Some(Ballot { id: proposal_id, value }),
        };
        return (StatusCode::OK, Json(payload));
    }

    println!("[/handle-prepare] Node {} accepted a new proposal: {}", state.node.id, proposal_id);

    let payload = HandleProposalPayload {
        error: None,
        value: Some(Ballot { id: proposal_id, value: None }),
    };

    (StatusCode::OK, Json(payload))
}

#[derive(Serialize, Deserialize, Debug)]
struct HandleAcceptPayload {
    error: Option<String>,
    value: Option<Ballot>,
}

async fn handle_accept(State(state): State<AppState>, propose: Json<Ballot>) -> (StatusCode, Json<HandleAcceptPayload>) {
    println!("[/handle-accept] Node {} get new propose to be accepted: {:?}", state.node.id, propose);

    let mut acceptor = state.acceptor.lock().await;
    if acceptor.last_ballot_number != propose.id {
        println!("[/handle-accept] Node {} received a proposal with a ballot ID different: {}", state.node.id, propose.id);
        let payload = HandleAcceptPayload {
            error: Some(String::from("Node received a proposal with a ballot ID different!")),
            value: None,
        };
        return (StatusCode::BAD_REQUEST, Json(payload));
    }

    println!("[/handle-accept] Node {} accepting new proposed value: {:?}", state.node.id, propose.value);

    acceptor.accepted_proposal = propose.value.clone();

    let payload = HandleAcceptPayload {
        error: None,
        value: Some(Ballot { id: propose.id, value: propose.value.clone() }),
    };

    (StatusCode::OK, Json(payload))
}

async fn handle_learn(State(state): State<AppState>, payload: Json<Ballot>) -> (StatusCode, ()) {
    let mut ledger = state.ledger.lock().await;

    // TODO: I'm not proud of it, but it works.
    match &payload.value {
        None => ledger.insert(payload.id, String::from("")),
        Some(v) => ledger.insert(payload.id, v.clone()),
    };

    println!("[/handle-learn] Node {} learns a new value: {:?}", state.node.id, payload.value);

    (StatusCode::OK, ())
}

#[derive(Serialize, Deserialize, Debug)]
struct Ballot {
    pub id: u64,
    pub value: Option<String>,
}

#[derive(Clone, Debug)]
struct Proposer {
    pub id: u64,
}

type Propose = Value;

impl Proposer {
    pub fn new() -> Self {
        Self { id: 0 }
    }

    pub async fn prepare(&mut self, state: &AppState, value: String) -> Result<Ballot, String> {
        let client = Client::new();

        self.id += 1;

        let nodes = state.nodes.lock().await;
        let reqs = nodes.iter().map(|node| {
            client.post(format!("http://{}/handle-prepare", node.addr))
                .json(&self.id)
                .send()
        });

        let responses = futures::future::join_all(reqs).await;

        let mut promises = Vec::with_capacity(responses.len());

        for response in responses.into_iter().flatten() {
            // TODO: Improve this to handle Err() stuffs.
            promises.push(response.json::<HandleProposalPayload>().await.unwrap());
        }

        let quorum = (nodes.len() / 2) + 1;

        if promises.len() < quorum {
            return Err(String::from("Proposal does not receive promises of the entire quorum"));
        }

        let accepted_promise = promises
            .into_iter()
            .filter(|promise| promise.value.is_some())
            .max_by_key(|promise| match &promise.value {
                None => 0,
                Some(value) => value.id,
            });

        // TODO: I'm not proud of this horrible stuff.
        let value = match accepted_promise {
            None => Some(value),
            Some(payload) => {
                match payload.value {
                    None => Some(value),
                    Some(ballot) => {
                        match ballot.value {
                            None => Some(value),
                            b => b,
                        }
                    },
                }
            }
        };

        let propose = Ballot { id: self.id, value };

        Ok(propose)
    }

    pub async fn propose(&self, state: &AppState, propose: &Ballot) -> Result<(), String> {
        let client = Client::new();

        let nodes = state.nodes.lock().await;

        let reqs = nodes.iter().map(|node| {
            client.post(format!("http://{}/handle-accept", node.addr))
                .json(&propose)
                .send()
        });

        let responses = futures::future::join_all(reqs).await;

        let mut accepted_ballots = Vec::with_capacity(responses.len());

        for response in responses.into_iter().flatten() {
            accepted_ballots.push(response.json::<HandleAcceptPayload>().await.unwrap());
        }

        let quorum = (nodes.len() / 2) + 1;

        if accepted_ballots.len() + 1 < quorum {
            return Err(String::from("Proposal not accepted by majority"));
        }

        Ok(())
    }
}
