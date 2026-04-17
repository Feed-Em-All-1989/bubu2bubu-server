mod noise;
mod protocol;
mod types;

use noise::build_responder;
use protocol::{send_frame, recv_frame, send_noise_msg, recv_noise_msg};
use types::{ClientMsg, ServerMsg};  
use noise::NoiseResponder;
use std::collections::HashMap;
use std::sync::Arc;
use tokio::sync::Mutex;
use tokio::net::TcpListener;
use tokio::net::tcp::OwnedWriteHalf;
use x25519_dalek::StaticSecret;
use rand::rngs::OsRng;
use rand::RngCore;

type Clients = Arc<Mutex<HashMap<u64, ClientEntry>>>;

struct ClientEntry {
    name: String,
    key_tag: String,
    avatar: Option<String>,
    transport: Arc<Mutex<NoiseResponder>>,
    writer: Arc<Mutex<OwnedWriteHalf>>,
}

fn collect_user_names(map: &HashMap<u64, ClientEntry>) -> Vec<String> {
    map.values().map(|e| e.name.clone()).collect()
}

fn generate_room_key() -> String {
    let mut bytes = [0u8; 32];
    OsRng.fill_bytes(&mut bytes);
    hex::encode(bytes)
}

#[tokio::main]
async fn main() {
    let port: u16 = std::env::args()
        .nth(1)
        .and_then(|s| s.parse().ok())
        .unwrap_or(9999);

    let secret = StaticSecret::random_from_rng(OsRng);
    let server_key: [u8; 32] = secret.to_bytes();
    let room_key = generate_room_key();

    let listener = TcpListener::bind(format!("0.0.0.0:{}", port))
        .await
        .expect("failed to bind");

    println!("server started on 0.0.0.0:{}", port);
    println!("room key: {}", room_key);

    let clients: Clients = Arc::new(Mutex::new(HashMap::new()));
    let mut next_id: u64 = 0;

    loop {
        let (stream, addr) = match listener.accept().await {
            Ok(s) => s,
            Err(_) => continue,
        };

        let id = next_id;
        next_id += 1;
        let clients = clients.clone();
        let key = server_key;
        let rk = room_key.clone();

        tokio::spawn(async move {
            if let Err(e) = handle_client(id, stream, &key, clients, &rk).await {
                eprintln!("client {} ({}) error: {}", id, addr, e);
            }
        });
    }
}

async fn handle_client(
    id: u64,
    mut stream: tokio::net::TcpStream,
    server_key: &[u8; 32],
    clients: Clients,
    room_key: &str,
) -> Result<(), String> {
    let mut responder = build_responder(server_key)?;

    let msg1 = recv_frame(&mut stream).await?;
    responder.read_message(&msg1)?;

    let msg2 = responder.write_message(&[])?;
    send_frame(&mut stream, &msg2).await?;

    let msg3 = recv_frame(&mut stream).await?;
    responder.read_message(&msg3)?;

    let noise = responder.into_transport()?;
    let (mut reader, writer) = stream.into_split();

    let transport = Arc::new(Mutex::new(noise));
    let writer = Arc::new(Mutex::new(writer));

    let join_data = recv_noise_msg(&mut reader, &transport).await?;
    let join_msg: ClientMsg = serde_json::from_slice(&join_data).map_err(|e| e.to_string())?;

    let mut client_name = match join_msg {
        ClientMsg::Join { name } => name,
        _ => return Err("expected join message".into()),
    };

    {
        let welcome = ServerMsg::Welcome { room_key: room_key.to_string() };
        let welcome_data = serde_json::to_vec(&welcome).unwrap();
        let mut w = writer.lock().await;
        send_noise_msg(&mut *w, &transport, &welcome_data).await?;
    }

    {
        let mut map = clients.lock().await;
        map.insert(id, ClientEntry {
            name: client_name.clone(),
            key_tag: String::new(),
            avatar: None,
            transport: transport.clone(),
            writer: writer.clone(),
        });
        let online = map.len();
        println!("{} connected ({} online)", client_name, online);

        let joined = ServerMsg::Joined { name: client_name.clone(), online };
        let data = serde_json::to_vec(&joined).unwrap();
        let user_list = ServerMsg::UserList { users: collect_user_names(&map) };
        let ul_data = serde_json::to_vec(&user_list).unwrap();
        for (&cid, entry) in map.iter() {
            if cid == id { continue; }
            let mut w = entry.writer.lock().await;
            let _ = send_noise_msg(&mut *w, &entry.transport, &data).await;
            let _ = send_noise_msg(&mut *w, &entry.transport, &ul_data).await;
        }
        let mut w = writer.lock().await;
        let _ = send_noise_msg(&mut *w, &transport, &ul_data).await;

        for entry in map.values() {
            if entry.key_tag.is_empty() { continue; }
            let profile = ServerMsg::ProfileUpdate {
                key_tag: entry.key_tag.clone(),
                name: entry.name.clone(),
                avatar: entry.avatar.clone(),
            };
            let pd = serde_json::to_vec(&profile).unwrap();
            let _ = send_noise_msg(&mut *w, &transport, &pd).await;
        }
    }

    loop {
        let raw = match recv_noise_msg(&mut reader, &transport).await {
            Ok(data) => data,
            Err(_) => break,
        };

        let msg: ClientMsg = match serde_json::from_slice(&raw) {
            Ok(m) => m,
            Err(_) => continue,
        };

        match msg {
            ClientMsg::Chat { id: msg_id, reply_to, image, metadata } => {
                let sender = {
                    let map = clients.lock().await;
                    map.get(&id).map(|e| e.name.clone()).unwrap_or_default()
                };
                let relay = ServerMsg::Chat {
                    sender,
                    id: msg_id,
                    reply_to,
                    image,
                    metadata,
                };
                broadcast(&clients, id, &relay).await;
            }
            ClientMsg::SetProfile { key_tag, name, avatar } => {
                let name_changed = {
                    let mut map = clients.lock().await;
                    let changed = if let Some(entry) = map.get_mut(&id) {
                        let changed = entry.name != name;
                        entry.key_tag = key_tag.clone();
                        entry.name = name.clone();
                        entry.avatar = avatar.clone();
                        if changed { client_name = name.clone(); }
                        changed
                    } else {
                        false
                    };
                    changed
                };
                let update = ServerMsg::ProfileUpdate { key_tag, name, avatar };
                broadcast_all(&clients, &update).await;
                if name_changed {
                    let map = clients.lock().await;
                    let user_list = ServerMsg::UserList { users: collect_user_names(&map) };
                    let ul_data = serde_json::to_vec(&user_list).unwrap();
                    for entry in map.values() {
                        let mut w = entry.writer.lock().await;
                        let _ = send_noise_msg(&mut *w, &entry.transport, &ul_data).await;
                    }
                }
            }
            _ => {}
        }
    }

    let online = {
        let mut map = clients.lock().await;
        map.remove(&id);
        map.len()
    };
    println!("{} disconnected ({} online)", client_name, online);

    let left = ServerMsg::Left { name: client_name, online };
    broadcast(&clients, id, &left).await;

    {
        let map = clients.lock().await;
        let user_list = ServerMsg::UserList { users: collect_user_names(&map) };
        let ul_data = serde_json::to_vec(&user_list).unwrap();
        for entry in map.values() {
            let mut w = entry.writer.lock().await;
            let _ = send_noise_msg(&mut *w, &entry.transport, &ul_data).await;
        }
    }

    Ok(())
}

async fn broadcast_all(clients: &Clients, msg: &ServerMsg) {
    let data = serde_json::to_vec(msg).unwrap();
    let targets: Vec<(Arc<Mutex<NoiseResponder>>, Arc<Mutex<OwnedWriteHalf>>)> = {
        let map = clients.lock().await;
        map.values()
            .map(|e| (e.transport.clone(), e.writer.clone()))
            .collect()
    };

    for (transport, writer) in targets {
        let mut w = writer.lock().await;
        let _ = send_noise_msg(&mut *w, &transport, &data).await;
    }
}

async fn broadcast(clients: &Clients, sender_id: u64, msg: &ServerMsg) {
    let data = serde_json::to_vec(msg).unwrap();
    let targets: Vec<(Arc<Mutex<NoiseResponder>>, Arc<Mutex<OwnedWriteHalf>>)> = {
        let map = clients.lock().await;
        map.iter()
            .filter(|(&cid, _)| cid != sender_id)
            .map(|(_, e)| (e.transport.clone(), e.writer.clone()))
            .collect()
    };

    for (transport, writer) in targets {
        let mut w = writer.lock().await;
        let _ = send_noise_msg(&mut *w, &transport, &data).await;
    }
}
