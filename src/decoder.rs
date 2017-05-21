use std::io::Read;
use std::result;
use std::str;
use serde::de::{Deserializer, Deserialize, DeserializeSeed, Visitor, VariantAccess, SeqAccess,
                MapAccess, EnumAccess};
use error::BencodeError;

pub type Result<T> = result::Result<T, BencodeError>;

pub struct BencodeVisitor<'a, R: 'a + Read> {
    de: &'a mut BencodeDecoder<R>,
}

impl<'a, R: 'a + Read> BencodeVisitor<'a, R> {
    fn new(de: &'a mut BencodeDecoder<R>) -> BencodeVisitor<'a, R> {
        BencodeVisitor { de: de }
    }
}

impl<'de, 'a, R: 'a + Read> VariantAccess<'de> for BencodeVisitor<'a, R> {
    type Error = BencodeError;

    fn newtype_variant_seed<T: DeserializeSeed<'de>>(self, seed: T) -> Result<T::Value> {
        seed.deserialize(self.de)
    }

    fn unit_variant(self) -> Result<()> {
        Err(BencodeError::UnknownVariant("Unit variant not supported.".into()))
    }

    fn tuple_variant<V: Visitor<'de>>(self, _: usize, _: V) -> Result<V::Value> {
        Err(BencodeError::UnknownVariant("Tuple variant not supported.".into()))
    }

    fn struct_variant<V: Visitor<'de>>(self, _: &'static [&'static str], _: V) -> Result<V::Value> {
        Err(BencodeError::UnknownVariant("Struct variant not supported.".into()))
    }
}

impl<'de, 'a, R: 'a + Read> SeqAccess<'de> for BencodeVisitor<'a, R> {
    type Error = BencodeError;

    fn next_element_seed<T: DeserializeSeed<'de>>(&mut self, seed: T) -> Result<Option<T::Value>> {
        self.de.update_state();
        match seed.deserialize(&mut *self.de) {
            Ok(v) => Ok(Some(v)),
            Err(_) => {
                self.de.state.pop();
                Ok(None)
            }
        }
    }
}

impl<'de, 'a, R: 'a + Read> MapAccess<'de> for BencodeVisitor<'a, R> {
    type Error = BencodeError;
    fn next_key_seed<K>(&mut self, seed: K) -> Result<Option<K::Value>>
        where K: DeserializeSeed<'de>
    {
        if self.de.state.last() == Some(&State::E) {
            return Ok(None);
        }
        self.de.update_state();
        match seed.deserialize(&mut *self.de) {
            Ok(v) => Ok(Some(v)),
            Err(_) => {
                self.de.state.pop();
                Ok(None)
            }
        }
    }

    fn next_value_seed<V>(&mut self, seed: V) -> Result<V::Value>
        where V: DeserializeSeed<'de>
    {
        self.de.update_state();
        seed.deserialize(&mut *self.de)
    }
}

impl<'de, 'a, R: 'a + Read> EnumAccess<'de> for BencodeVisitor<'a, R> {
    type Error = BencodeError;
    type Variant = Self;
    fn variant_seed<V: DeserializeSeed<'de>>(self, seed: V) -> Result<(V::Value, Self)> {
        let res = seed.deserialize(&mut *self.de)?;
        Ok((res, self))
    }
}

#[derive(PartialEq, Debug)]
enum State {
    S(Vec<u8>),
    I(i64),
    L,
    D,
    E,
}

#[derive(Debug)]
pub struct BencodeDecoder<R: Read> {
    reader: R,
    state: Vec<State>,
    is_struct: bool,
    is_option: bool,
}

impl<'de, R: Read> BencodeDecoder<R> {
    pub fn new(reader: R) -> BencodeDecoder<R> {
        BencodeDecoder {
            reader: reader,
            state: vec![],
            is_struct: false,
            is_option: false,
        }
    }

    fn parse_int(&mut self) -> Result<State> {
        let mut buf = [0; 1];
        let mut result = String::new();
        while self.reader.read(&mut buf).unwrap() != 0 {
            match str::from_utf8(&buf) {
                Ok("e") => {
                    return match result.parse::<i64>() {
                               Ok(i) => Ok(State::I(i)),
                               Err(_) => {
                                   Err(BencodeError::InvalidValue(format!("Can't parse `{}` as i64",
                                                                          result)))
                               }
                           }
                }
                Ok(c) => result.push_str(&c),
                Err(_) => {
                    return Err(BencodeError::InvalidValue("Non UTF-8 integer encoding".to_string()))
                }
            }
        }
        Err(BencodeError::EndOfStream)
    }

    fn parse_byte_string_body(&mut self, len: i64) -> Result<Vec<u8>> {
        let mut buf = [0; 1];
        let mut result = Vec::new();
        for _ in 0..len {
            assert!(self.reader.read(&mut buf).unwrap() != 0);
            result.push(buf[0]);
        }
        Ok(result)
    }

    fn parse_byte_string_len(&mut self, len_char: char) -> Result<i64> {
        let mut buf = [0; 1];
        let mut len = String::new();
        len.push(len_char);
        loop {
            match self.reader.read(&mut buf) {
                Ok(1) => {
                    match String::from_utf8(buf.to_vec()) {
                        Ok(c) => {
                            match c.as_str() {
                                ":" => {
                                    match len.parse::<i64>() {
                                        Ok(len) => return Ok(len),
                                        Err(_) => {
                                            return Err(BencodeError::InvalidValue(format!("Can't parse `{}` as string length",
                                                                                          len)))
                                        }
                                    }
                                }
                                n => len.push_str(n),
                            }
                        }
                        Err(_) => {
                            return Err(BencodeError::InvalidValue("Non UTF-8 integer encoding"
                                                                      .to_string()))
                        }
                    }
                }
                _ => return Err(BencodeError::EndOfStream),
            }
        }
    }

    fn parse_byte_string(&mut self, len_char: char) -> Result<State> {
        match self.parse_byte_string_len(len_char) {
            Ok(len) => {
                match self.parse_byte_string_body(len) {
                    Ok(b) => Ok(State::S(b)),
                    Err(e) => Err(e),
                }
            }
            Err(e) => Err(e),
        }
    }

    fn parse_state(&mut self) -> Result<State> {
        let mut buf = [0; 1];
        if 1 == self.reader.read(&mut buf).unwrap() {
            match buf[0].into() {
                'l' => Ok(State::L),
                'd' => Ok(State::D),
                'e' => Ok(State::E),
                'i' => self.parse_int(),
                n @ '0'...'9' => self.parse_byte_string(n),
                _ => Err(BencodeError::EndOfStream),
            }
        } else {
            Err(BencodeError::EndOfStream)
        }
    }

    fn update_state(&mut self) {
        match self.parse_state() {
            Ok(s) => self.state.push(s),
            _ => (),
        }
    }
}

impl<'de, 'a, R: Read> Deserializer<'de> for &'a mut BencodeDecoder<R> {
    type Error = BencodeError;

    #[inline]
    fn deserialize_any<V: Visitor<'de>>(mut self, visitor: V) -> Result<V::Value> {
        if self.state.last() == None {
            self.update_state();
        }
        if self.is_option {
            self.is_option = false;
            visitor.visit_some(self)
        } else {
            match self.state.pop() {
                Some(State::I(i)) => visitor.visit_i64(i),
                Some(State::S(s)) => visitor.visit_byte_buf(s),
                Some(State::L) => visitor.visit_seq(BencodeVisitor::new(&mut self)),
                Some(State::D) => visitor.visit_map(BencodeVisitor::new(&mut self)),
                _ => Err(BencodeError::EndOfStream),
            }
        }
    }

    forward_to_deserialize_any! {
        i64 string seq bool i8 i16 i32 u8 u16 u32
        u64 f32 f64 char str unit bytes byte_buf map unit_struct tuple_struct tuple
        newtype_struct ignored_any
    }

    #[inline]
    fn deserialize_option<V: Visitor<'de>>(self, visitor: V) -> Result<V::Value> {
        self.is_option = true;
        if self.state.last() == None {
            self.update_state();
        }
        self.deserialize_any(visitor)
    }

    #[inline]
    fn deserialize_struct<V: Visitor<'de>>(self,
                                           _name: &'static str,
                                           _fields: &'static [&'static str],
                                           visitor: V)
                                           -> Result<V::Value> {
        self.is_struct = true;
        if self.state.last() == None {
            self.update_state();
        }
        visitor.visit_map(BencodeVisitor::new(self))
    }

    #[inline]
    fn deserialize_identifier<V: Visitor<'de>>(self, visitor: V) -> Result<V::Value> {
        if self.is_struct {
            match self.state.last() {
                Some(&State::S(ref b)) => visitor.visit_bytes(b),
                _ => Err(BencodeError::EndOfStream),
            }
        } else {
            self.is_struct = true;
            match self.state.last() {
                Some(&State::I(_)) => visitor.visit_str("Integer"),
                Some(&State::S(_)) => visitor.visit_str("ByteString"),
                Some(&State::D) => visitor.visit_str("Dict"),
                Some(&State::L) => visitor.visit_str("List"),
                _ => Err(BencodeError::EndOfStream),
            }
        }
    }

    #[inline]
    fn deserialize_enum<V: Visitor<'de>>(self,
                                         _enum: &'static str,
                                         _variants: &'static [&'static str],
                                         visitor: V)
                                         -> Result<V::Value> {
        self.is_struct = false;
        if self.state.last() == None {
            self.update_state();
        }
        visitor.visit_enum(BencodeVisitor::new(self))
    }
}

pub fn from_str<'de, T>(s: &'de str) -> Result<T>
    where T: Deserialize<'de>
{
    self::from_bytes(s.as_bytes())
}

pub fn from_bytes<'de, T>(b: &'de [u8]) -> Result<T>
    where T: Deserialize<'de>
{
    Deserialize::deserialize(&mut BencodeDecoder::new(b))
}
