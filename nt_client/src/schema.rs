//! Schema management for `struct`s and/or `protobuf`s.

use std::{collections::HashMap, convert::Infallible, fmt::Debug, sync::Arc};

#[cfg(feature = "protobuf")]
use protobuf::{MessageDyn, MessageFull, descriptor::FileDescriptorProto, reflect::{FileDescriptor, MessageDescriptor}};
use tokio::sync::{Mutex, broadcast};
use tracing::{debug, warn};

#[cfg(feature = "protobuf")]
use crate::protobuf::ProtobufData;

#[cfg(feature = "struct")]
use crate::r#struct::{StructData, StructSchema, byte::ByteReader, parse::{ParsedStruct, StructValue, parse_schema}};

use crate::{ClientHandle, data::{DataType, NetworkTableData}, error::ConnectionClosedError, publish::NewPublisherError, subscribe::{ReceivedMessage, SubscriptionOptions}, topic::Properties};

/// A clonable schema manager.
///
/// Clones will share the same internal schema map.
#[derive(Debug, Clone)]
pub struct SchemaManager {
    #[cfg(feature = "struct")]
    structs: Arc<Mutex<StructSchemas>>,
    #[cfg(feature = "protobuf")]
    protos: Arc<Mutex<ProtobufSchemas>>,
    handle: ClientHandle,
}

impl SchemaManager {
    pub(crate) fn new(handle: ClientHandle) -> Self {
        Self {
            #[cfg(feature = "struct")]
            structs: Arc::new(Mutex::new(StructSchemas::new())),
            #[cfg(feature = "protobuf")]
            protos: Arc::new(Mutex::new(ProtobufSchemas::new())),
            handle,
        }
    }

    /// Publishes the struct schema for `T`, as well as any nested structs that `T` references.
    ///
    /// The published topic is set to `retained` by default.
    ///
    /// Nothing is published if a schema for `T` has already been parsed.
    #[cfg(feature = "struct")]
    pub async fn publish_struct<T: StructData>(&mut self) -> Result<(), PublishSchemaError> {
        {
            let schemas = self.structs.lock().await;
            if schemas.has_schema(&T::struct_type_name()) {
                return Ok(());
            }
        }

        T::publish_dependencies(self).await?;

        let publisher = self.handle.struct_schema_topic::<T>().publish::<StructSchema>(Properties { retained: Some(true), ..Default::default() }).await?;
        publisher.set_default(T::schema()).await?;
        Ok(())
    }

    /// Publishes the protobuf schema for `T`, as well as any nested protobufs `T` depends on.
    ///
    /// The published topic is set to `retained` by default.
    ///
    /// Nothing is published if a schema for `T` has already been parsed.
    #[cfg(feature = "protobuf")]
    pub async fn publish_proto<T: ProtobufData>(&mut self) -> Result<(), PublishSchemaError> {
        self.publish_file_descriptor(T::message_descriptor().file_descriptor()).await
    }

    #[cfg(feature = "protobuf")]
    async fn publish_file_descriptor(&mut self, descriptor: &FileDescriptor) -> Result<(), PublishSchemaError> {
        {
            let schemas = self.protos.lock().await;
            if schemas.has_schema(descriptor.name()) {
                return Ok(());
            }
        }

        for dep in descriptor.deps() {
            Box::pin(self.publish_file_descriptor(dep)).await?;
        }

        let publisher = self.handle.protobuf_schema_topic(descriptor).publish::<FileDescriptorProto>(Properties { retained: Some(true), ..Default::default() }).await?;
        publisher.set_default(descriptor.proto().clone()).await?;
        // publisher is retained, so we can drop it here
        Ok(())
    }

    /// Parses a `NetworkTables` value as a schema-defined struct.
    ///
    /// # Errors
    ///
    /// Returns an error if a matching schema wasn't found or the value could not be parsed.
    #[cfg(feature = "struct")]
    pub async fn parse_struct(&mut self, type_name: &str, value: rmpv::Value) -> Result<Vec<(String, StructValue)>, ParseFromSchemaError> {
        let bytes = value.as_slice().ok_or(ParseFromSchemaError::InvalidData)?;
        self.structs.lock().await.parse(type_name, bytes)
    }

    /// Parses a `NetworkTables` value as a schema-defined protobuf message.
    ///
    /// # Errors
    ///
    /// Returns an error if a matching schema wasn't found or the value could not be parsed.
    #[cfg(feature = "protobuf")]
    pub async fn parse_proto(&mut self, type_name: &str, value: rmpv::Value) -> Result<Box<dyn MessageDyn>, ParseFromSchemaError> {
        let bytes = value.as_slice().ok_or(ParseFromSchemaError::InvalidData)?;
        self.protos.lock().await.parse(type_name, bytes)
    }

    /// Watches the `/.schema/` topic, parsing and adding any schemas it encounters to its shared
    /// schema map.
    ///
    /// Specific schemas types will only be parsed from enabled features, e.g. if only the `struct`
    /// feature is enabled only `struct` schemas will be recognized and parsed.
    ///
    /// Note that this method will only return if it encounters an error (notice the [`Infallible`]
    /// `Ok` type).
    pub async fn watch(self) -> Result<Infallible, broadcast::error::RecvError> {
        let mut sub = self.handle.schema_topic().subscribe(SubscriptionOptions { prefix: Some(true), ..Default::default() }).await.map_err(|_| broadcast::error::RecvError::Closed)?;
        loop {
            if let ReceivedMessage::Updated((topic, value)) = sub.recv().await? {
                let type_name = topic.name().strip_prefix("/.schema/").expect("/.schema/ prefix");

                match topic.r#type() {
                    #[cfg(feature = "struct")]
                    DataType::StructSchema => {
                        match type_name.strip_prefix("struct:") {
                            Some(type_name) => self.structs.lock().await.insert_struct_schema(type_name.to_owned(), value),
                            None => warn!("[schema struct:{type_name}] expected struct schema to start with `struct:`"),
                        }
                    },
                    #[cfg(feature = "protobuf")]
                    DataType::Protobuf(proto) if proto == FileDescriptorProto::descriptor().name() => {
                        match type_name.strip_prefix("proto:") {
                            Some(type_name) => self.protos.lock().await.insert_proto_schema(type_name, value),
                            None => warn!("[schema proto:{type_name}] expected protobuf schema to start with `proto:`"),
                        }
                    },
                    r#type => warn!("[schema proto:{type_name}] invalid schema type {type:?}"),
                }
            }
        }
    }
}

/// NetworkTables `struct` schemas.
#[cfg(feature = "struct")]
#[derive(Default, Debug, Clone, PartialEq, Eq)]
pub struct StructSchemas {
    schemas: HashMap<String, ParsedStruct>,
    listeners: Vec<StructDepListener>,
}

#[cfg(feature = "struct")]
impl StructSchemas {
    /// Creates a new, empty map of schemas.
    pub fn new() -> Self {
        Default::default()
    }

    /// Returns if the schema for a type exists.
    pub fn has_schema(&self, type_name: &str) -> bool {
        self.schemas.contains_key(type_name)
    }

    /// Gets the parsed struct for a type.
    pub fn get(&self, type_name: &str) -> Option<&ParsedStruct> {
        self.schemas.get(type_name)
    }

    /// Parses byte data as a struct.
    ///
    /// # Errors
    ///
    /// Returns an error if the schema for the struct was not found or if the bytes could not be
    /// parsed.
    pub fn parse(&self, type_name: &str, bytes: &[u8]) -> Result<Vec<(String, StructValue)>, ParseFromSchemaError> {
        self.get(type_name)
            .ok_or(ParseFromSchemaError::SchemaNotFound)
            .and_then(|parsed_struct| parsed_struct.read_from_bytes(&mut ByteReader::new(bytes), &self.schemas)
                .ok_or(ParseFromSchemaError::InvalidData))
    }

    /// Inserts a new struct schema based on its type name and a published NetworkTables value.
    pub fn insert_struct_schema(&mut self, type_name: String, value: rmpv::Value) {
        match StructSchema::from_value(value).and_then(|schema| parse_schema(&schema.0).ok()) {
            Some(schema) => {
                let missing_deps: Vec<_> = schema.deps.iter()
                    .filter(|dep| !self.schemas.contains_key(*dep))
                    .cloned()
                    .collect();
                if missing_deps.is_empty() {
                    self.insert_schema(type_name, schema);
                } else {
                    debug!("[schema struct:{type_name}] waiting for missing dependencies {missing_deps:?}");
                    self.listeners.push(StructDepListener { missing_deps, type_name, parsed: schema });
                }
            },
            None => warn!("[schema struct:{type_name}] invalid struct schema"),
        }
    }

    fn insert_schema(&mut self, type_name: String, parsed: ParsedStruct) {
        debug!("[schema struct:{type_name}] parsed as {parsed:?}");

        // PERF: no alloc here?
        let new_listeners: Box<[_]> = self.listeners.extract_if(.., |listener| listener.add_dep(&type_name)).collect();
        for new_listener in new_listeners {
            self.insert_schema(new_listener.type_name, new_listener.parsed);
        }

        self.schemas.insert(type_name, parsed);
    }
}

#[cfg(feature = "struct")]
#[derive(Debug, Clone, PartialEq, Eq)]
struct StructDepListener {
    pub missing_deps: Vec<String>,
    pub type_name: String,
    pub parsed: ParsedStruct,
}

#[cfg(feature = "struct")]
impl StructDepListener {
    fn add_dep(&mut self, added: &str) -> bool {
        self.missing_deps.retain(|dep| dep != added);
        self.missing_deps.is_empty()
    }
}

/// NetworkTables `protobuf` schemas.
#[cfg(feature = "protobuf")]
#[derive(Default, Debug, Clone, PartialEq)]
pub struct ProtobufSchemas {
    schemas: HashMap<String, MessageDescriptor>,
    deps: Vec<FileDescriptor>,
    listeners: Vec<ProtobufDepListener>,
}

#[cfg(feature = "protobuf")]
impl ProtobufSchemas {
    /// Creates a new, empty map of schemas.
    pub fn new() -> Self {
        Default::default()
    }

    /// Returns if the schema for a type exists.
    pub fn has_schema(&self, type_name: &str) -> bool {
        self.schemas.contains_key(type_name)
    }

    /// Gets the parsed struct for a type.
    pub fn get(&self, type_name: &str) -> Option<&MessageDescriptor> {
        self.schemas.get(type_name)
    }

    /// Parses byte data as a protobuf message.
    ///
    /// # Errors
    ///
    /// Returns an error if the schema for the message was not found or if the bytes could not be
    /// parsed.
    pub fn parse(&self, type_name: &str, bytes: &[u8]) -> Result<Box<dyn MessageDyn>, ParseFromSchemaError> {
        self.get(type_name)
            .ok_or(ParseFromSchemaError::SchemaNotFound)
            .and_then(|descriptor| descriptor.parse_from_bytes(bytes)
                .map_err(|_| ParseFromSchemaError::InvalidData))
    }

    /// Inserts protobuf schema(s) based on its type name and a published NetworkTables value.
    pub fn insert_proto_schema(&mut self, type_name: &str, value: rmpv::Value) {
        match FileDescriptorProto::from_value(value) {
            Some(file_descriptor) => {
                let missing_deps: Vec<_> = file_descriptor.dependency.iter()
                    .filter(|dep| !self.deps.iter().any(|descriptor| descriptor.name() == *dep))
                    .cloned()
                    .collect();
                if missing_deps.is_empty() {
                    self.insert_schema(type_name, file_descriptor);
                } else {
                    debug!("[schema proto:{type_name}] waiting for missing dependencies {missing_deps:?}");
                    self.listeners.push(ProtobufDepListener { type_name: type_name.to_owned(), descriptor: file_descriptor, missing_deps });
                }
            },
            None => warn!("[schema proto:{type_name}] invalid protobuf schema"),
        }
    }

    fn insert_schema(&mut self, type_name: &str, descriptor: FileDescriptorProto) {
        let fd = match FileDescriptor::new_dynamic(descriptor, &self.deps) {
            Ok(fd) => fd,
            Err(err) => {
                warn!("[schema proto:{type_name}] unable to parse file descriptor: {err}");
                return;
            }
        };
        for descriptor in fd.messages() {
            let name = descriptor.full_name().to_owned();
            debug!("[schema proto:{type_name}] parsed message as {descriptor:?}");
            self.schemas.insert(name, descriptor);
        }

        let new_listeners: Box<_> = self.listeners.extract_if(.., |listener| listener.add_dep(fd.name())).collect();
        self.deps.push(fd);
        for new_listener in new_listeners {
            self.insert_schema(&new_listener.type_name, new_listener.descriptor);
        }
    }
}

#[cfg(feature = "protobuf")]
#[derive(Debug, Clone, PartialEq)]
struct ProtobufDepListener {
    pub type_name: String,
    pub descriptor: FileDescriptorProto,
    pub missing_deps: Vec<String>,
}

#[cfg(feature = "protobuf")]
impl ProtobufDepListener {
    pub fn add_dep(&mut self, added: &str) -> bool {
        self.missing_deps.retain(|dep| dep != added);
        self.missing_deps.is_empty()
    }
}

/// Errors that can occur when publishing a schema.
#[derive(thiserror::Error, Debug, Clone, PartialEq, Eq)]
pub enum PublishSchemaError {
    /// An error occurred when creating a new publisher.
    #[error(transparent)]
    NewPublisher(#[from] NewPublisherError),

    /// The `NetworkTables` connection was closed.
    #[error(transparent)]
    ConnectionClosed(#[from] ConnectionClosedError),
}

/// Errors that can occur when parsing a piece of data from a schema.
#[derive(thiserror::Error, Debug, Clone, PartialEq)]
pub enum ParseFromSchemaError {
    /// The associated schema was not found.
    #[error("the schema was not found")]
    SchemaNotFound,

    /// The data is invalid and could not be parsed.
    #[error("invalid data")]
    InvalidData,
}

