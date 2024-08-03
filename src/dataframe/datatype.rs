use serde::{de::Visitor, Deserialize, Deserializer, Serialize, Serializer};

// holy macro
macro_rules! impl_data_type {
    // T (and vec<T>) to data type with from<data type> -> T impl
    ($t: ty $([vec => $a: expr])? => $d: expr ; $v: ident @ $f: expr) => {
        impl_data_type!(@ $t, $d, [$v]{ $f });
        $( impl_data_type!(vec $t => $a; $v @ { $f }); )?
    };

    // T (and vec<T>) to data type with from<data type> -> T impl (block)
    ($t: ty $([vec => $a: expr])? => $d: expr ; $v: ident @ $f: block) => {
        impl_data_type!(@ $t, $d, [$v]$f);
        $( impl_data_type!(vec $t => $a; $v @ { $f }); )?
    };

    // int (and vec<int>) to data type
    ($t: ty $([vec => $a: expr])? => $d: expr ; i) => {
        impl_data_type!(@ $t, $d, [value] value.as_i64());
        $( impl_data_type!(vec $t => $a; value @ { value.as_i64().and_then(|value| value.try_into().ok()) }); )?
    };
    // uint (and vec<uint>) to data type
    ($t: ty $([vec => $a: expr])? => $d: expr ; u) => {
        impl_data_type!(@ $t, $d, [value] value.as_u64());
        $( impl_data_type!(vec $t => $a; value @ { value.as_u64().and_then(|value| value.try_into().ok()) }); )?
    };

    // some_wrapper(vec<T>) to data type with mapper from value to T
    (vec($i: ty) $t: ty => $d: expr ; $v: ident @ $c: block) => {
        impl_data_type!(@ $t, $d, [value]{
            let vec = value.as_array()?;
            vec.iter()
                .map(|$v| $c)
                .collect::<Option<$t>>()
        }, [this]{
            let vec: Vec<$i> = this.into();
            rmpv::Value::from_iter(vec)
        });
    };
    // vec<T> to data type with mapper from value to T
    (vec $i: ty => $d: expr ; $v: ident @ $c: block) => {
        impl_data_type!(@ Vec<$i>, $d, [value]{
            let vec = value.as_array()?;
            vec.iter()
                .map(|$v| $c)
                .collect::<Option<Vec<$i>>>()
        }, [this]{ rmpv::Value::from_iter(this) });
    };
    // some_wrapper(vec<u8>) to data type
    (bytes $t: ty => $d: expr) => {
        impl_data_type!(vec(u8) $t => $d ; value @ {
            value.as_u64().and_then(|value| value.try_into().ok())
        });
    };

    // INTERNAL generic impl with some from logic without custom into logic
    (@ $t: ty, $d: expr, [ $v: ident ] $a: expr) => {
        impl_data_type!(@ $t, $d, [$v]{ $a.and_then(|value| value.try_into().ok()) }, [this]{ this.into() });
    };
    // INTERNAL generic impl with custom from and into logic
    (@ $t: ty, $d: expr, [ $v: ident ] $f: block, [ $s: ident ] $i: block) => {
        impl NetworkTableDataType for $t {
            fn data_type() -> DataType {
                use DataType::*;
                $d
            }
            #[allow(unused_variables)]
            fn from_value($v: &rmpv::Value) -> Option<Self> {
                $f
            }
            fn into_value(self) -> rmpv::Value {
                let $s = self;
                $i
            }
        }
    };
}

macro_rules! transparent {
    ($t: ident : $g: ty) => {
        transparent!(@ $t, $g);
    };
    ($t: ident : vec $g: ty) => {
        transparent!(@ $t, Vec<$g>);
        transparent!(@vec $t, $g);
    };

    (@ $t: ident, $g: ty) => {
        #[derive(Clone)]
        pub struct $t(pub $g);
        impl From<$t> for $g {
            fn from(value: $t) -> Self {
                value.0
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
    (@vec $t: ident, $i: ty) => {
        impl FromIterator<$i> for $t {
            fn from_iter<I: IntoIterator<Item = $i>>(iter: I) -> Self {
                iter.into_iter().collect::<Vec<$i>>().into()
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
transparent!(RawData: vec u8);
transparent!(Rpc: vec u8);
transparent!(Protobuf: vec u8);

impl_data_type!(bool [vec => BooleanArray] => Boolean; value @ value.as_bool());
impl_data_type!(f64 [vec => DoubleArray] => Double; value @ value.as_f64());
impl_data_type!(i8 [vec => IntArray] => Int; i);
impl_data_type!(i16 [vec => IntArray] => Int; i);
impl_data_type!(i32 [vec => IntArray] => Int; i);
impl_data_type!(i64 [vec => IntArray] => Int; value @ value.as_i64());
impl_data_type!(u8 [vec => IntArray] => Int; u);
impl_data_type!(u16 [vec => IntArray] => Int; u);
impl_data_type!(u32 [vec => IntArray] => Int; u);
impl_data_type!(u64 [vec => IntArray] => Int; value @ value.as_u64());
impl_data_type!(f32 [vec => FloatArray] => Float; value @ value.as_f64().map(|num| num as f32));
impl_data_type!(String [vec => StringArray] => String; value @ value.as_str().map(|str| str.to_owned()));
impl_data_type!(JsonString => Json; value @ value.as_str().map(|str| JsonString(str.to_owned())));
impl_data_type!(bytes RawData => Raw);
impl_data_type!(bytes Rpc => Rpc);
impl_data_type!(rmpv::Value => Msgpack; value @ Some(value.clone()));
impl_data_type!(bytes Protobuf => Protobuf);

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

