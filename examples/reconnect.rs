//! An example of using a reconnect handler.

use nt_client::{data::r#type::NetworkTableData, error::ReconnectError, subscribe::ReceivedMessage};

#[tokio::main]
async fn main() {
    // creates a reconnect handler with the NewClientOptions of Default::default()
    nt_client::reconnect(Default::default(), |client| async {
        // handle everything in this closure

        // subscribes to topic `/value`
        let sub_topic = client.topic("/value");
        let sub_task = tokio::spawn(async move {
            let mut subscriber = sub_topic.subscribe(Default::default()).await;

            loop {
                match subscriber.recv().await {
                    Ok(ReceivedMessage::Updated((topic, value))) => {
                        let value = String::from_value(&value).expect("updated value is a string");
                        println!("topic {} updated to {value}", topic.name());
                    },
                    Err(err) => return Err(ReconnectError::Nonfatal(err.into())),
                    _ => {},
                }
            }
        });

        // select! to make sure tokio tasks don't keep running
        tokio::select! {
            // fatal error if the client encounters an IoError, nonfatal otherwise
            res = client.connect() => Ok(res?),
            // fatal error if the task encountered an error while joining,
            // otherwise return a nonfatal error
            res = sub_task => res
                .map_err(|err| ReconnectError::Fatal(err.into()))?
                .map_err(|err| ReconnectError::Nonfatal(err.into())),
        }
    }).await.unwrap()
    // unwrap the fatal error (if there is one)
}

