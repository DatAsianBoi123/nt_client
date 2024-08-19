//! An example of using both a publisher and a subscriber with NetworkTables.

use std::time::Duration;

use nt_client::{data::{r#type::NetworkTableData, SubscriptionOptions}, subscribe::ReceivedMessage, Client, NTAddr, NewClientOptions};
use tracing::level_filters::LevelFilter;

#[tokio::main]
async fn main() {
    tracing_subscriber::fmt()
        .with_max_level(LevelFilter::INFO)
        .init();

    let client = Client::new(NewClientOptions { 
        addr: NTAddr::Local,
        // custom WSL ip
        // addr: NTAddr::Custom(Ipv4Addr::new(172, 30, 64, 1)),
        ..Default::default()
    });

    // subscribes to `/topic` and prints all changes to stdout
    // changes include the announcement of a topic, an updated value, and an unannouncement of a topic
    let sub_topic = client.topic("/topic");
    tokio::spawn(async move {
        let mut subscriber = sub_topic.subscribe(SubscriptionOptions { prefix: Some(true), ..Default::default() }).await;

        loop {
            match subscriber.recv().await {
                Ok(ReceivedMessage::Announced(topic)) => println!("announced topic: {}", topic.name()),
                Ok(ReceivedMessage::Updated((topic, value))) => {
                    let value = String::from_value(&value).expect("updated value is a string");
                    println!("topic {} updated to {value}", topic.name());
                },
                Ok(ReceivedMessage::Unannounced { name, .. }) => {
                    println!("topic {name} unannounced");
                },
                Err(_) => break,
            }
        }
    });

    // publishes to `/counter` and increments its value by 1 every second
    let pub_topic = client.topic("/counter");
    tokio::spawn(async move {
        let publisher = pub_topic.publish::<u32>(Default::default()).await.expect("can publish to topic");
        let mut counter = 0;

        publisher.set_default(counter).await;

        loop {
            tokio::time::sleep(Duration::from_secs(1)).await;
            counter += 1;

            println!("updated counter to {counter}");
            publisher.set(counter).await;
        }
    });

    client.connect().await.unwrap();
}

