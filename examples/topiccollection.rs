//! An example of using a TopicCollection to subscribe to multiple topics.

use nt_client::{subscribe::ReceivedMessage, Client};

#[tokio::main]
async fn main() {
    let client = Client::new(Default::default());

    let topic_names = vec!["/topic1".to_owned(), "/topic2".to_owned(), "topic3".to_owned()];
    let topics = client.topics(topic_names.clone());

    // loop over topics in the collection
    for topic in topics.clone() {
        println!("{topic:?}");

        // do something with topic
    }

    tokio::spawn(async move {
        // subscribe to `/topic1`, `/topic2`, and `/topic3`
        let mut sub = topics.subscribe(Default::default()).await;

        loop {
            match sub.recv().await {
                Ok(ReceivedMessage::Updated((topic, value))) => println!("topic {} updated to {value:?}", topic.name()),
                Err(_) => break,
                _ => {},
            }
        }
    });

    client.connect().await.unwrap();
}

