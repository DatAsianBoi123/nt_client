//! An example of using a reconnect handler.

use nt_client::{data::r#type::NetworkTableData, subscribe::ReceivedMessage, Client};

#[tokio::main]
async fn main() -> Result<(), std::io::Error> {
    nt_client::reconnect(|| async {
        let client = Client::new(Default::default());

        let sub_topic = client.topic("/value");
        let sub_task = tokio::spawn(async move {
            let mut subscriber = sub_topic.subscribe(Default::default()).await;

            loop {
                match subscriber.recv().await {
                    Ok(ReceivedMessage::Updated((topic, value))) => {
                        let value = String::from_value(&value).expect("updated value is a string");
                        println!("topic {} updated to {value}", topic.name());
                    },
                    Err(err) => {
                        eprint!("{err:?}");
                        break;
                    },
                    _ => {},
                }
            }
        });

        tokio::select! {
            res = client.connect() => res,
            _ = sub_task => Ok(()),
        }
    }).await
}

