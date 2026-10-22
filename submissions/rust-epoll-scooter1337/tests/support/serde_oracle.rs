use serde::Deserialize;
use std::borrow::Cow;
#[derive(Deserialize)]
struct Header<'a> {
    #[serde(borrow)]
    alg: Cow<'a, str>,
}
#[derive(Deserialize)]
struct Claims<'a> {
    #[serde(borrow)]
    sub: Option<&'a serde_json::value::RawValue>,
    #[serde(borrow)]
    username: Option<&'a serde_json::value::RawValue>,
    exp: Option<f64>,
    nbf: Option<f64>,
}
#[derive(Deserialize)]
#[serde(transparent)]
struct Text<'a>(#[serde(borrow)] Cow<'a, str>);
// Visit the complete JSON document, keeping only the last top-level body string.
// Plain strings borrow the request. Escaped strings use serde's checked decoder.
// Ignored fields are still validated, including Unicode and nested containers.
#[derive(Clone, Copy)]
enum BodySeed {
    Root,
    String,
    Ignore,
}
impl<'de> serde::de::DeserializeSeed<'de> for BodySeed {
    type Value = Option<Cow<'de, str>>;
    fn deserialize<D: serde::Deserializer<'de>>(self, d: D) -> Result<Self::Value, D::Error> {
        d.deserialize_any(self)
    }
}
impl<'de> serde::de::Visitor<'de> for BodySeed {
    type Value = Option<Cow<'de, str>>;
    fn expecting(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str("a JSON value")
    }
    fn visit_borrowed_str<E: serde::de::Error>(self, s: &'de str) -> Result<Self::Value, E> {
        Ok(matches!(self, Self::String).then_some(Cow::Borrowed(s)))
    }
    fn visit_str<E: serde::de::Error>(self, s: &str) -> Result<Self::Value, E> {
        Ok(if matches!(self, Self::String) {
            Some(Cow::Owned(s.to_owned()))
        } else {
            None
        })
    }
    fn visit_unit<E: serde::de::Error>(self) -> Result<Self::Value, E> {
        Ok(None)
    }
    fn visit_bool<E: serde::de::Error>(self, _: bool) -> Result<Self::Value, E> {
        Ok(None)
    }
    fn visit_i64<E: serde::de::Error>(self, _: i64) -> Result<Self::Value, E> {
        Ok(None)
    }
    fn visit_u64<E: serde::de::Error>(self, _: u64) -> Result<Self::Value, E> {
        Ok(None)
    }
    fn visit_f64<E: serde::de::Error>(self, _: f64) -> Result<Self::Value, E> {
        Ok(None)
    }
    fn visit_seq<A: serde::de::SeqAccess<'de>>(self, mut seq: A) -> Result<Self::Value, A::Error> {
        while seq.next_element_seed(Self::Ignore)?.is_some() {}
        Ok(None)
    }
    fn visit_map<A: serde::de::MapAccess<'de>>(self, mut map: A) -> Result<Self::Value, A::Error> {
        let mut body = None;
        if matches!(self, Self::Root) {
            while let Some(Text(key)) = map.next_key::<Text<'de>>()? {
                let value = map.next_value_seed(if key == "body" {
                    Self::String
                } else {
                    Self::Ignore
                })?;
                if key == "body" {
                    body = value;
                }
            }
        } else {
            while map.next_key_seed(Self::Ignore)?.is_some() {
                map.next_value_seed(Self::Ignore)?;
            }
        }
        Ok(body)
    }
}
struct PostBody<'a>(Option<Cow<'a, str>>);
impl<'de> Deserialize<'de> for PostBody<'de> {
    fn deserialize<D: serde::Deserializer<'de>>(d: D) -> Result<Self, D::Error> {
        use serde::de::DeserializeSeed;
        BodySeed::Root.deserialize(d).map(Self)
    }
}

pub fn header(bytes: &[u8]) -> Result<(), serde_json::Error> {
    let header: Header = serde_json::from_slice(bytes)?;
    assert_eq!(header.alg, "HS256");
    Ok(())
}
pub fn body(bytes: &[u8]) -> Result<Option<Cow<'_, str>>, serde_json::Error> {
    serde_json::from_slice::<PostBody>(bytes).map(|b| b.0)
}

pub fn identity(bytes: &[u8]) -> Result<(Cow<'_, str>, Cow<'_, str>, f64, Option<f64>), ()> {
    let c: Claims = serde_json::from_slice(bytes).map_err(|_| ())?;
    let sub: Text = serde_json::from_str(c.sub.ok_or(())?.get()).map_err(|_| ())?;
    let name: Text = serde_json::from_str(c.username.ok_or(())?.get()).map_err(|_| ())?;
    Ok((sub.0, name.0, c.exp.ok_or(())?, c.nbf))
}
