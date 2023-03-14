use std::mem::size_of;

use zerocopy::{AsBytes, FromBytes, LayoutVerified, Unaligned};

macro_rules! types {
    ($($type: ident),* $(,)*) => {
        $(type $type = ::zerocopy::byteorder::$type<::zerocopy::byteorder::NetworkEndian>;)*
    };
}

types![U16, U32, U64];

#[derive(Clone, Copy, Debug, FromBytes, AsBytes, Unaligned)]
#[repr(transparent)]
struct Offset(pub U16);

impl Offset {
    pub fn from_index<T>(index: usize) -> Option<Offset> {
        Some(Offset(U16::new((size_of::<T>() + index).try_into().ok()?)))
    }

    pub fn to_index<T>(self) -> Option<usize> {
        (self.0.get() as usize).checked_sub(size_of::<T>())
    }
}

macro_rules! id {
    ($package: ident, $id: literal) => {
        impl $package {
            pub const ID: u16 = $id;
        }
    };
}

#[derive(Debug, FromBytes, AsBytes, Unaligned)]
#[repr(C)]
pub struct Prelude {
    pub version: U64,
}

#[derive(Debug, FromBytes, AsBytes, Unaligned)]
#[repr(C)]
pub struct Header {
    pub kind: U16,
    pub length: U16,
}

#[derive(Debug, FromBytes, AsBytes, Unaligned)]
#[repr(C)]
pub struct Ping {
    pub id: U64,
}
id!(Ping, 0);

#[derive(Debug, FromBytes, AsBytes, Unaligned)]
#[repr(C)]
pub struct Pong {
    pub id: U64,
}
id!(Pong, 1);

#[derive(Debug, FromBytes, AsBytes, Unaligned)]
#[repr(C)]
pub struct ClientInfo {
    os: Offset,
    os_version: Offset,
    release: Offset,
}
id!(ClientInfo, 2);

pub struct ClientInfoRef<'buffer> {
    pub os: &'buffer str,
    pub os_version: &'buffer str,
    pub release: &'buffer str,
}

impl ClientInfo {
    pub fn write<'buffer>(
        buffer: &'buffer mut [u8],
        os: &str,
        os_version: &str,
        release: &str,
    ) -> Option<&'buffer [u8]> {
        let (offsets, data) = buffer.split_at_mut(size_of::<Self>());

        let mut i = 0;

        let mut field = move |field: &[u8]| {
            let offset = Offset::from_index::<Self>(i)?;
            data[i..i + field.len()].copy_from_slice(field);
            i += field.len();
            Some(offset)
        };

        let mut offsets = LayoutVerified::<_, Self>::new(offsets);
        let offsets = offsets.as_mut()?;

        offsets.os = field(os.as_bytes())?;
        offsets.os_version = field(os_version.as_bytes())?;
        offsets.release = field(release.as_bytes())?;

        Some(&mut buffer[..size_of::<Self>() + i])
    }

    pub fn read(buffer: &[u8]) -> Option<ClientInfoRef<'_>> {
        let (offsets, data) = buffer.split_at(size_of::<Self>());

        let offsets = LayoutVerified::<_, Self>::new(offsets);
        let offsets = offsets.as_ref()?;

        let os_index = offsets.os.to_index::<Self>()?;
        let os_version_index = offsets.os_version.to_index::<Self>()?;
        let release_index = offsets.release.to_index::<Self>()?;

        Some(ClientInfoRef {
            os: std::str::from_utf8(&data[os_index..os_version_index]).ok()?,
            os_version: std::str::from_utf8(&data[os_version_index..release_index]).ok()?,
            release: std::str::from_utf8(&data[release_index..]).ok()?,
        })
    }
}

pub struct CodecList {}
id!(CodecList, 3);

impl CodecList {
    pub fn read(buffer: &[u8]) -> Option<&[Codec]> {
        Some(LayoutVerified::new_slice(buffer)?.into_slice())
    }

    pub fn write<'buffer>(
        buffer: &'buffer mut [u8],
        codecs: &[Codec],
    ) -> Option<&'buffer mut [u8]> {
        let buffer = buffer.get_mut(..codecs.len() * size_of::<Codec>())?;

        LayoutVerified::new_slice(&mut *buffer)?
            .into_mut_slice()
            .copy_from_slice(codecs);

        Some(buffer)
    }
}

#[derive(Copy, Clone, Debug, FromBytes, AsBytes, Unaligned)]
#[repr(transparent)]
pub struct Codec(pub U16);

#[derive(Copy, Clone, Debug, FromBytes, AsBytes, Unaligned)]
#[repr(C)]
pub struct StreamSetup {
    pub stream: U16,
    pub codec: Codec,
    pub sample_rate: U16,
    pub channels: u8,
}
id!(StreamSetup, 4);

pub struct StreamData {
    pub stream: U16,
    pub sample_id: U16,
}
id!(StreamData, 5);

pub struct ClientQOS {
    pub ping: U16,
    pub udp_state: u8,

    pub package_loss_perc: u8,

    pub samples_sent: U32,
    pub samples_received: U32,

    pub sent_samples_lost: U32,
    pub received_samples_lost: U32,
}
id!(ClientQOS, 6);
