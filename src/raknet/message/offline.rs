use crate::raknet::datatypes::{ReadBuf, WriteBuf};

use super::{write_header, Message, MessageError, RaknetMessage};

#[derive(Clone, Debug)]
pub struct MessageUnconnectedPing {
    pub client_uuid: i64,
    pub forward_timestamp: i64,
}

#[derive(Clone, Debug)]
pub struct MessageUnconnectedPong {
    pub timestamp: i64,
    pub server_uuid: i64,
    pub motd: String,
}

impl Message for MessageUnconnectedPing {
    fn serialize(&self, buf: &mut WriteBuf) -> Result<(), MessageError> {
        write_header(buf, RaknetMessage::UnconnectedPing)?;
        buf.write_i64(self.forward_timestamp)?;
        buf.write_magic()?;
        buf.write_i64(self.client_uuid)?;
        Ok(())
    }

    fn deserialize(buf: &mut ReadBuf) -> Result<Self, MessageError> {
        let timestamp = buf.read_i64()?;
        buf.read_magic()?;
        Ok(Self {
            forward_timestamp: timestamp,
            client_uuid: buf.read_i64()?,
        })
    }
}

impl Message for MessageUnconnectedPong {
    fn serialize(&self, buf: &mut WriteBuf) -> Result<(), MessageError> {
        write_header(buf, RaknetMessage::UnconnectedPong)?;
        buf.write_i64(self.timestamp)?;
        buf.write_i64(self.server_uuid)?;
        buf.write_magic()?;
        buf.write_str(&self.motd)?;
        Ok(())
    }

    fn deserialize(buf: &mut ReadBuf) -> Result<Self, MessageError> {
        let timestamp = buf.read_i64()?;
        let server_uuid = buf.read_i64()?;
        buf.read_magic()?;
        let motd = buf.read_str()?;
        Ok(Self {
            timestamp,
            server_uuid,
            motd,
        })
    }
}
