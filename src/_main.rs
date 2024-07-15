#[macro_use]
extern crate rocket;

use rocket::fs::FileServer;
use rocket::response::stream::{Event, EventStream};
use rocket::serde::{Deserialize, Serialize};
use rocket::tokio::select;
use rocket::tokio::sync::broadcast::{channel, error::RecvError, Sender};
use rocket::{get, routes, Shutdown, State};
use std::error::Error;
use std::time;

#[get("/check")]
async fn check(queue: &State<Sender<Message>>) {
    let one_sec = time::Duration::from_secs(1);
    let mut counter = 0;

    loop {
        counter += 1;
        if counter == 6 {
            break;
        }

        queue
            .send(Message {
                text: "Trying to connect to host".to_string(),
            })
            .unwrap();

        tokio::time::sleep(one_sec).await;
    }
}

#[derive(Debug, Clone, FromForm, Serialize, Deserialize, Default)]
#[cfg_attr(test, derive(PartialEq, UriDisplayQuery))]
#[serde(crate = "rocket::serde")]
struct Message {
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
        .mount("/api", routes![check])
        .mount("/", FileServer::from("public"))
        .manage(channel::<Message>(1024).0)
        .launch()
        .await?;

    Ok(())
}
