#[macro_use]
extern crate rocket;

use anyhow::Result;
use chrono::Local;
use response::HostResource;
use rocket::fs::FileServer;
use rocket::http::Status;
use rocket::response::stream::{Event, EventStream};
use rocket::serde::json::{json, Json, Value};
use rocket::serde::{Deserialize, Serialize};
use rocket::tokio::select;
use rocket::tokio::sync::broadcast::{channel, error::RecvError, Sender};
use rocket::{get, post, routes, Shutdown, State};
use ssh2::{ErrorCode, Session};
use std::error::Error;
use std::fs::File;
use std::io::{prelude::*, ErrorKind};
use std::net::{SocketAddr, TcpStream};
use std::str::FromStr;
use std::time::Duration;
use toml_edit::DocumentMut;
use validator::Validate;

mod config;
mod response;
mod validation;


fn parse_hosts_file(file: &str) -> Vec<HostResource> {
    let mut file = File::open(file).unwrap();
    let mut contents = String::new();
    file.read_to_string(&mut contents).unwrap();
    let conf = contents.parse::<DocumentMut>().expect("invalid config");
    let mut hosts: Vec<HostResource> = vec![];

    // host level
    conf.iter().enumerate().for_each(|(_i, (key, host))| {
        if key == "host" {
            // host_name level
            host.as_table()
                .unwrap()
                .iter()
                .enumerate()
                .for_each(|(_i, (_host_name, host))| {
                    let ip = host["ip"].as_str().unwrap();
                    let port = host["port"].as_str().unwrap();
                    let user = host["user"].as_str().unwrap();
                    let pass = host["pass"].as_str().unwrap();

                    hosts.push(HostResource {
                        ip: ip.to_string(),
                        port: port.to_string(),
                        user: user.to_string(),
                        pass: pass.to_string(),
                    });
                });
        }
    });

    hosts
}

#[get("/hosts")]
async fn get_hosts() -> response::GetHostsResponder {
    response::GetHostsResponder {
        inner: Json(parse_hosts_file("config/installer.toml")),
    }
}

#[post("/hosts", data = "<input>")]
async fn post_hosts(input: Json<Vec<validation::PostHosts>>) -> response::PostHostsResponder {
    match input.validate() {
        Err(e) => {
            return response::PostHostsResponder::ValidationErrors(json!(
                response::ResponseBag::new_with_errors(Some(e.errors()))
            ));
        }
        Ok(_) => {
            config::save_as_toml("config/installer.toml", input).unwrap();

            return response::PostHostsResponder::Saved {
                inner: Json(vec![]),
            };
        }
    }
}

#[post("/ping")]
async fn ping(queue: &State<Sender<Message>>) -> (Status, Value) {
    let hosts = parse_hosts_file("config/installer.toml");

    for host in &hosts {
        queue
            .send(Message {
                what: "log".to_string(),
                ts: Local::now().format("%H:%M:%S%.3f").to_string(),
                r#type: Some("info".to_string()),
                text: format!(
                    "Trying to connect to host {}:{}. User {} and password {}...",
                    host.ip, host.port, host.user, host.pass
                ),
            })
            .unwrap();

        let addr = SocketAddr::from_str(format!("{}:{}", host.ip, host.port).as_str()).unwrap();
        let tcp_result = TcpStream::connect_timeout(&addr, Duration::new(3, 0));
        let tcp = match tcp_result {
            Ok(tcp) => tcp,
            Err(e) => match e.kind() {
                ErrorKind::TimedOut => {
                    queue
                        .send(Message {
                            what: "log".to_string(),
                            ts: Local::now().format("%H:%M:%S%.3f").to_string(),
                            r#type: Some("error".to_string()),
                            text: format!(
                                "Can't connect to host {}:{}. Reason: Timeout (3s).",
                                host.ip, host.port
                            ),
                        })
                        .unwrap();
                    continue;
                }
                _ => {
                    return (Status::InternalServerError, json!({}));
                }
            },
        };

        let mut sess = Session::new().unwrap();
        sess.set_tcp_stream(tcp);
        sess.handshake().unwrap();

        let auth_result = sess.userauth_password(host.user.as_str(), host.pass.as_str());
        match auth_result {
            Err(e) => match e.code() {
                ErrorCode::Session(code) => {
                    match code {
                        -18 => {
                            queue
                                .send(Message {
                                    what: "log".to_string(),
                                    ts: Local::now().format("%H:%M:%S%.3f").to_string(),
                                    r#type: Some("error".to_string()),
                                    text: format!(
                                "Can't connect to host {}:{}. Reason: bad credentials.",
                                host.ip, host.port
                            ),
                                })
                                .unwrap();
                        }
                        _ => {
                            queue
                                .send(Message {
                                    what: "log".to_string(),
                                    ts: Local::now().format("%H:%M:%S%.3f").to_string(),
                                    r#type: Some("error".to_string()),
                                    text: format!(
                                    "Can't connect to host {}:{}. Reason: unknown error.",
                                    host.ip, host.port
                                ),
                                })
                                .unwrap();
                        }
                    }

                    continue;
                }
                _ => {
                    return (Status::InternalServerError, json!({}));
                }
            },
            _ => {}
        };

        if sess.authenticated() {
            queue
                .send(Message {
                    what: "log".to_string(),
                    ts: Local::now().format("%H:%M:%S%.3f").to_string(),
                    r#type: Some("success".to_string()),
                    text: format!("Connected to host {}:{} successfully.", host.ip, host.port),
                })
                .unwrap();
        } else {
            queue
                .send(Message {
                    what: "log".to_string(),
                    ts: Local::now().format("%H:%M:%S%.3f").to_string(),
                    r#type: Some("error".to_string()),
                    text: format!("Can't connect to host {}:{}.", host.ip, host.port),
                })
                .unwrap();
        }
    }

    (Status::Ok, json!({}))
}

#[derive(Debug, Clone, FromForm, Serialize, Deserialize, Default)]
#[cfg_attr(test, derive(PartialEq, UriDisplayQuery))]
#[serde(crate = "rocket::serde")]
struct Message {
    pub what: String,
    pub ts: String,
    pub r#type: Option<String>,
    pub text: String,
}

/// Returns an infinite stream of server-sent events. Each event is a message
/// pulled from a broadcast queue sent by the `post` handler.
#[get("/")]
async fn sse_events(queue: &State<Sender<Message>>, mut end: Shutdown) -> EventStream![] {
    let mut rx = queue.subscribe();
    EventStream! {
        loop {
            let msg = select! {
                msg = rx.recv() => match msg {
                    Ok(msg) => msg,
                    Err(RecvError::Closed) => break,
                    Err(RecvError::Lagged(_)) => continue,
                },
                _ = &mut end => break,
            };

            yield Event::json(&msg);
        }
    }
}

#[rocket::main]
async fn main() -> Result<(), Box<dyn Error>> {
    // dotenvy::dotenv()?;

    let _ = rocket::build()
        .mount("/sse", routes![sse_events])
        .mount("/api", routes![get_hosts, post_hosts, ping])
        .mount("/", FileServer::from("public"))
        .manage(channel::<Message>(1024).0)
        .launch()
        .await?;

    Ok(())
}
