//! An example of using both a publisher and a subscriber with NetworkTables.

use std::time::Duration;

use nt_client::{Client, NTAddr, NewClientOptions, data::{DataType, NetworkTableData}, subscribe::ReceivedMessage};
use tracing::level_filters::LevelFilter;

#[tokio::main]
async fn main() {
    tracing_subscriber::fmt()
        .with_max_level(LevelFilter::INFO)
        .init();

    let client = Client::new(NewClientOptions { 
        // if connecting to a real robot, use `NTAddr::TeamNumber` instead.
        addr: NTAddr::Local,
        secure_port: None,
        ..Default::default()
    });

    client.connect_setup(setup).await.unwrap();
}

fn setup(client: &Client) {
    // subscribes to `/topic` and prints all changes to stdout
    // changes include the announcement of a topic, an updated value, and an unannouncement of a topic
    let sub_topic = client.topic("/topic");
    tokio::spawn(async move {
        let mut subscriber = sub_topic.subscribe(Default::default()).await.unwrap();

        loop {
            match subscriber.recv().await {
                Ok(ReceivedMessage::Announced(topic)) => println!("announced topic: {}", topic.name()),
                Ok(ReceivedMessage::Updated((topic, value))) => {
                    match topic.r#type() {
                        DataType::String => println!("topic {} updated to {}", topic.name(), String::from_value(value).expect("is a valid string")),
                        _ => eprintln!("not a string"),
                    }
                },
                Ok(ReceivedMessage::Unannounced { name, .. }) => {
                    println!("topic {name} unannounced");
                },
                Ok(ReceivedMessage::UpdateProperties(topic)) => {
                    println!("topic {} updated its properties to {:?}", topic.name(), topic.properties());
                }
                Err(err) => {
                    eprint!("{err:?}");
                    break;
                },
            }
        }
    });

    // publishes to `/counter` and increments its value by 1 every second
    let pub_topic = client.topic("/counter");
    tokio::spawn(async move {
        let publisher = pub_topic.publish::<u32>(Default::default()).await.expect("can publish to topic");
        let mut counter = 0;

        publisher.set_default(counter).await.expect("connection is still alive");

        loop {
            tokio::time::sleep(Duration::from_secs(1)).await;
            counter += 1;

            println!("updated counter to {counter}");
            publisher.set(counter).await.expect("connection is still alive");
        }
    });
}

