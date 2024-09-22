//! An example of using a reconnect handler.

use std::error::Error;

use nt_client::{data::r#type::NetworkTableData, error::ReconnectError, subscribe::ReceivedMessage};

#[tokio::main]
async fn main() -> Result<(), Box<dyn Error + Send + Sync>> {
    nt_client::reconnect(Default::default(), |client| async {
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

        tokio::select! {
            res = client.connect() => Ok(res?),
            res = sub_task => res
                .map_err(|err| ReconnectError::Fatal(err.into()))?
                .map_err(|err| ReconnectError::Nonfatal(err.into())),
        }
    }).await
}

