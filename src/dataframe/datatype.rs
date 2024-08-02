use std::ops::{Deref,DerefMut};

use serde::{de::Visitor, Deserialize, Deserializer, Serialize, Serializer};

macro_rules! impl_data_type {
    (($v: ident) $($t: ty => $d: expr ; $f: expr),+ $(,)?) => {
        $(
            impl NetworkTableDataType for $t {
                fn data_type() -> DataType {
                    use DataType::*;
                    $d
                }
                fn from_value(value: &rmpv::Value) -> Option<Self> {
                    #[allow(unused_variables)]
                    let $v = value;
                    $f
                }
                fn into_value(self) -> rmpv::Value {
                    self.into()
                }
            }
        )+
    };
}

macro_rules! impl_data_type_vec {
    (($v: ident) $($t: ty => $d: expr ; $f: expr),+ $(,)?) => {
        $(
            impl NetworkTableDataType for $t {
                fn data_type() -> DataType {
                    use DataType::*;
                    $d
                }
                fn from_value(value: &rmpv::Value) -> Option<Self> {
                    #[allow(unused_variables)]
                    let $v = value;
                    $f
                }
                fn into_value(self) -> rmpv::Value {
                    rmpv::Value::from_iter(self)
                }
            }
        )+
    };
}

macro_rules! transparent {
    ($t: ident : $g: ty) => {
        #[derive(Clone)]
        pub struct $t($g);
        impl Deref for $t {
            type Target = $g;
            fn deref(&self) -> &Self::Target {
                &self.0
            }
        }
        impl DerefMut for $t {
            fn deref_mut(&mut self) -> &mut Self::Target {
                &mut self.0
            }
        }

        impl From<$g> for $t {
            fn from(value: $g) -> Self {
                Self(value)
            }
        }

        #[allow(clippy::from_over_into)]
        impl Into<rmpv::Value> for $t {
            fn into(self) -> rmpv::Value {
                self.0.into()
            }
        }
    };
}

#[derive(Serialize, Deserialize, Debug, Clone, PartialEq, Eq)]
#[serde(rename_all = "lowercase")]
pub enum DataType {
    Boolean,
    Double,
    Int,
    Float,
    String,
    Json,
    Raw,
    Rpc,
    Msgpack,
    Protobuf,
    #[serde(rename = "boolean[]")]
    BooleanArray,
    #[serde(rename = "double[]")]
    DoubleArray,
    #[serde(rename = "int[]")]
    IntArray,
    #[serde(rename = "float[]")]
    FloatArray,
    #[serde(rename = "string[]")]
    StringArray,
}

impl DataType {
    pub fn from_id(id: u32) -> Option<Self> {
        use DataType as D;

        match id {
            0 => Some(D::Boolean),
            1 => Some(D::Double),
            2 => Some(D::Int),
            3 => Some(D::Float),
            4 => Some(D::String),
            5 => Some(D::Raw),
            16 => Some(D::BooleanArray),
            17 => Some(D::DoubleArray),
            18 => Some(D::IntArray),
            19 => Some(D::FloatArray),
            20 => Some(D::StringArray),

            _ => None,
        }
    }

    pub fn as_id(&self) -> u32 {
        use DataType as D;

        match self {
            D::Boolean => 0,
            D::Double => 1,
            D::Int => 2,
            D::Float => 3,
            D::String | D::Json => 4,
            D::Raw | D::Rpc | D::Msgpack | D::Protobuf => 5,
            D::BooleanArray => 16,
            D::DoubleArray => 17,
            D::IntArray => 18,
            D::FloatArray => 19,
            D::StringArray => 20,
        }
    }
}

pub trait NetworkTableDataType: Clone {
    fn data_type() -> DataType;

    fn from_value(value: &rmpv::Value) -> Option<Self>;

    fn into_value(self) -> rmpv::Value;
}

transparent!(JsonString: String);
transparent!(RawData: Vec<u8>);
transparent!(Rpc: Vec<u8>);
// TODO: actual protobuf support
transparent!(Protobuf: Vec<u8>);

impl_data_type! [(value)
    bool => Boolean; value.as_bool(),
    f64 => Double; value.as_f64(),
    i8 => Int; value.as_i64().and_then(|num| num.try_into().ok()),
    i16 => Int; value.as_i64().and_then(|num| num.try_into().ok()),
    i32 => Int; value.as_i64().and_then(|num| num.try_into().ok()),
    i64 => Int; value.as_i64(),
    u8 => Int; value.as_u64().and_then(|num| num.try_into().ok()),
    u16 => Int; value.as_u64().and_then(|num| num.try_into().ok()),
    u32 => Int; value.as_u64().and_then(|num| num.try_into().ok()),
    u64 => Int; value.as_u64(),
    f32 => Float; value.as_f64().map(|num| num as f32),
    String => String; value.as_str().map(|str| str.to_owned()),
    // &str => String; value.as_str(),
    JsonString => Json; value.as_str().map(|str| str.to_owned().into()),
    // TODO: fix binary from_value impls
    RawData => Raw; None,
    Rpc => Rpc; None,
    rmpv::Value => Msgpack; Some(value.clone()),
    Protobuf => Protobuf; None,
];
// TODO: fix array from_value impls
impl_data_type_vec! [(value)
    Vec<bool> => BooleanArray; None,
    Vec<f64> => DoubleArray; None,
    Vec<i32> => IntArray; None,
    Vec<f32> => FloatArray; None,
    Vec<String> => StringArray; None,
];

pub fn serialize_as_u32<S>(data_type: &DataType, serializer: S) -> Result<S::Ok, S::Error>
where S: Serializer
{
    serializer.serialize_u32(data_type.as_id())
}

pub fn deserialize_u32<'de, D>(deserializer: D) -> Result<DataType, D::Error>
where D: Deserializer<'de>
{
    deserializer.deserialize_u32(DataTypeVisitor)
}

pub(super) struct DataTypeVisitor;

impl<'de> Visitor<'de> for DataTypeVisitor {
    type Value = DataType;

    fn expecting(&self, formatter: &mut std::fmt::Formatter) -> std::fmt::Result {
        write!(formatter, "a valid type id")
    }

    fn visit_i64<E>(self, v: i64) -> Result<Self::Value, E>
    where E: serde::de::Error
    {
        self.visit_u64(v.try_into().map_err(E::custom)?)
    }

    fn visit_u64<E>(self, v: u64) -> Result<Self::Value, E>
    where E: serde::de::Error
    {
        DataType::from_id(v.try_into().map_err(E::custom)?).ok_or(E::custom(format!("{v} is not a valid type id")))
    }
}

