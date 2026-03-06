//! An example of schema parsing and the schema manager using the "struct" and "protobuf" features.

use std::time::Duration;

use nt_client::{Client, data::DataType, schema::{ParseFromSchemaError, PublishSchemaError, SchemaManager}, r#struct::{StructData, StructSchema, byte::{ByteBuffer, ByteReader}}, subscribe::ReceivedMessage, topic::Properties};
use protobuf::reflect::ReflectFieldRef;

// create a struct to send to the server
#[derive(Debug, Clone, Copy, PartialEq)]
struct Position {
    x: f64,
    y: f64,
}

impl StructData for Position {
    fn struct_type_name() -> String {
        "Position".to_string()
    }

    fn schema() -> StructSchema {
        StructSchema("double x;double y".to_string())
    }

    async fn publish_dependencies(_manager: &mut SchemaManager) -> Result<(), PublishSchemaError> {
        // we don't publish anything here because our struct definition is just primitives
        // if we referenced other structs, we'd have to `manager.publish_struct` each one
        Ok(())
    }

    fn pack(self, buf: &mut ByteBuffer) {
        // this has to match with the schema definition we wrote earlier
        buf.write_f64(self.x);
        buf.write_f64(self.y);
    }

    fn unpack(read: &mut ByteReader) -> Option<Self> {
        // this has to match with the schema definition we wrote earlier
        Some(Self {
            x: read.read_f64()?,
            y: read.read_f64()?,
        })
    }
}

#[tokio::main]
async fn main() {
    let client = Client::new(Default::default());

    client.connect_setup(setup).await.unwrap()
}

fn setup(client: &Client) {
    // create a new schema manager
    let mut manager = client.schema_manager();

    // cloned manager still shares the same internal schema map
    let watch_manager = manager.clone();
    tokio::spawn(async move {
        // watch for schemas in `/.schema/`
        watch_manager.watch().await.unwrap()
    });

    let mut pub_manager = manager.clone();
    let struct_topic = client.topic("/position");
    tokio::spawn(async move {
        // publish our Position schema
        pub_manager.publish_struct::<Position>().await.unwrap();

        // we use retained here to make sure the value does not disappear once the publisher is
        // dropped
        let publisher = struct_topic.publish(Properties { retained: Some(true), ..Default::default() }).await.unwrap();

        let position = Position {
            x: 15.2,
            y: 8.91,
        };

        publisher.set(position.into_struct_data()).await.unwrap();
    });

    let mut sub_struct_manager = manager.clone();
    // read structs from "/struct"
    let sub_struct_topic = client.topic("/struct");
    tokio::spawn(async move {
        // subscribe to "/struct"
        let mut sub = sub_struct_topic.subscribe(Default::default()).await.unwrap();

        while let Ok(message) = sub.recv().await {
            if let ReceivedMessage::Updated((topic, value)) = message
                && let DataType::Struct(type_name) = topic.r#type() {
                // parse the struct from a schema that should have been caught by
                // `watch_manager.watch()`
                match sub_struct_manager.parse_struct(type_name, value).await {
                    Ok(fields) => {
                        println!("{type_name} {{");
                        for (name, value) in fields {
                            println!("  {name}: {value:?}");
                        }
                        println!("}}");
                    },
                    Err(ParseFromSchemaError::SchemaNotFound) => eprintln!("the schema for struct:{type_name} was not found"),
                    Err(ParseFromSchemaError::InvalidData) => eprintln!("invalid struct"),
                }
            }
        }
    });

    // read protobufs from "/protobuf"
    let sub_proto_topic = client.topic("/protobuf");
    tokio::spawn(async move {
        // subscribe to "/protobuf"
        let mut sub = sub_proto_topic.subscribe(Default::default()).await.unwrap();

        while let Ok(message) = sub.recv().await {
            if let ReceivedMessage::Updated((topic, value)) = message
                && let DataType::Protobuf(type_name) = topic.r#type() {
                // parse the protobuf message from a schema that should have been caught by
                // `watch_manager.watch()`
                tokio::time::sleep(Duration::from_millis(100)).await;
                match manager.parse_proto(type_name, value).await {
                    Ok(message) => {
                        let descriptor = message.descriptor_dyn();
                        println!("{} {{", descriptor.name());
                        for field in descriptor.fields() {
                            let value = field.get_reflect(&*message);
                            match value {
                                ReflectFieldRef::Map(map) => println!("  {}: {map:#?}", field.name()),
                                ReflectFieldRef::Optional(optional) => println!("  {}: {:#?}", field.name(), optional.value()),
                                ReflectFieldRef::Repeated(repeated) => println!("  {}: {repeated:#?}", field.name()),
                            }
                        }
                        println!("}}");
                    },
                    Err(ParseFromSchemaError::SchemaNotFound) => eprintln!("the schema for proto:{type_name} was not found"),
                    Err(ParseFromSchemaError::InvalidData) => eprintln!("invalid protobuf"),
                }
            }
        }
    });
}

