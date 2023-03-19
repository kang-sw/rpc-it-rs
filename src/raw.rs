use std::{mem::size_of, ops::Range};

use enum_primitive_derive::Primitive;
use num_traits::FromPrimitive;

/// Every message starts with this header
pub const MAX_ROUTE_LEN: usize = 1 << 10;
pub const PKT_TYPE_OFFSET: usize = 30;
pub const PKT_TYPE_MASK: u32 = 0b11;
pub const ROUTE_LEN_MASK: u32 = 0x3ff;
pub const ERRC_MASK: u32 = 0xff;
pub const ROUTE_LEN_OFFSET: usize = 20;
pub const ERRC_OFFSET: usize = 8;

/* ---------------------------------------------------------------------------------------------- */
/*                                       RAW MESSAGE HEADER                                       */
/* ---------------------------------------------------------------------------------------------- */
#[repr(C)]
#[derive(Clone, Copy, Debug)]
pub struct RawHead {
    b0: u32,
    p: u32,
}

pub type RawHeadBuf = [u8; size_of::<RawHead>()];

#[cfg(feature = "id-128bit")]
pub type IdType = u128;
#[cfg(not(feature = "id-128bit"))]
pub type IdType = u64;

pub const ID_MAX_LEN: usize = size_of::<IdType>();
pub const REQ_ID_LEN_MASK: u32 = size_of::<IdType>() as u32 - 1;
pub const REQ_ID_LEN_OFFSET: usize = 16;

impl RawHead {
    fn is_le() -> bool {
        const ENDIAN_CHECK: *const i32 = &1 as *const i32;
        unsafe { *(ENDIAN_CHECK as *const u8) == 1 }
    }

    pub fn from_bytes(mut value: RawHeadBuf) -> Self {
        // Rearranges frames received in network byte order to system byte order, if necessary.
        if Self::is_le() {
            value[0..4].reverse();
            value[4..8].reverse();
        }

        unsafe { std::mem::transmute(value) }
    }

    pub fn to_bytes(&self) -> RawHeadBuf {
        let mut value: RawHeadBuf = unsafe { std::mem::transmute_copy(self) };

        // Rearranges frames received in network byte order to system byte order, if necessary.
        if Self::is_le() {
            value[0..4].reverse();
            value[4..8].reverse();
        }

        value
    }

    pub fn new(this: Head) -> Self {
        match this {
            Head::Noti(HNoti {
                n_route: n,
                n_all: p,
            }) => Self {
                b0: 0b00 << PKT_TYPE_OFFSET | (n as u32 & ROUTE_LEN_MASK) << ROUTE_LEN_OFFSET,
                p,
            },
            Head::Req(HReq {
                n_route: n,
                n_req_id: m,
                n_all: p,
            }) => Self {
                b0: 0b01 << PKT_TYPE_OFFSET
                    | (n as u32 & ROUTE_LEN_MASK) << ROUTE_LEN_OFFSET
                    | (m as u32 & REQ_ID_LEN_MASK) << REQ_ID_LEN_OFFSET,
                p,
            },
            Head::Rep(HRep {
                errc: e,
                n_req_id: m,
                n_all: p,
            }) => Self {
                b0: 0b10 << PKT_TYPE_OFFSET
                    | (m as u32 & REQ_ID_LEN_MASK) << REQ_ID_LEN_OFFSET
                    | (e as u32) << ERRC_OFFSET,
                p,
            },
        }
    }

    pub fn parse(&self) -> Result<Head, ParseError> {
        let t = (self.b0 >> PKT_TYPE_OFFSET) & PKT_TYPE_MASK;
        let n = (self.b0 >> ROUTE_LEN_OFFSET) & ROUTE_LEN_MASK;
        let m = (self.b0 >> REQ_ID_LEN_OFFSET) & REQ_ID_LEN_MASK;
        let e = (self.b0 >> ERRC_OFFSET) & ERRC_MASK;

        match t {
            0 => Ok(Head::Noti(HNoti {
                n_route: n as u16,
                n_all: self.p,
            })),
            1 => Ok(Head::Req(HReq {
                n_route: n as u16,
                n_req_id: m as u8,
                n_all: self.p,
            })),
            2 => Ok(Head::Rep(HRep {
                errc: ReplyCode::from_u8(e as u8).ok_or(ParseError::InvalidErrCode(e as u8))?,
                n_req_id: m as u8,
                n_all: self.p,
            })),
            ty => Err(ParseError::InvalidType(ty as u8)),
        }
    }
}

#[derive(thiserror::Error, Debug)]
pub enum ParseError {
    #[error("invalid message type: {0}")]
    InvalidType(u8),

    #[error("invalid error code: {0}")]
    InvalidErrCode(u8),
}

/* ---------------------------------------------------------------------------------------------- */
/*                                          PARSED HEADER                                         */
/* ---------------------------------------------------------------------------------------------- */
#[derive(Clone, Copy, Debug, derive_more::From)]
pub enum Head {
    Noti(HNoti),
    Req(HReq),
    Rep(HRep),
}

/* --------------------------------------------- -- --------------------------------------------- */
#[derive(Clone, Copy, Debug)]
pub struct HNoti {
    pub n_route: u16,
    pub n_all: u32,
}

impl HNoti {
    pub fn n(&self) -> usize {
        self.n_route as usize
    }

    pub fn p(&self) -> usize {
        self.n_all as usize
    }

    pub fn route(&self) -> Range<usize> {
        0..self.n()
    }

    // pub fn route_c(&self) -> Range<usize> {
    //     0..self.n() + 1
    // }

    pub fn payload(&self) -> Range<usize> {
        self.n() + 1..self.p()
    }

    pub fn payload_c(&self) -> Range<usize> {
        self.n() + 1..self.p() + 1
    }
}

/* --------------------------------------------- -- --------------------------------------------- */
#[derive(Clone, Copy, Debug)]
pub struct HReq {
    pub n_route: u16,
    pub n_req_id: u8,
    pub n_all: u32,
}

impl HReq {
    pub fn n(&self) -> usize {
        self.n_route as usize
    }

    pub fn m(&self) -> usize {
        self.n_req_id as usize
    }

    pub fn p(&self) -> usize {
        self.n_all as usize
    }

    pub fn route(&self) -> Range<usize> {
        self.m()..self.m() + self.n()
    }

    // pub fn route_c(&self) -> Range<usize> {
    //     self.m()..self.m() + self.n() + 1
    // }

    pub fn payload(&self) -> Range<usize> {
        self.m() + self.n() + 1..self.p()
    }

    pub fn payload_c(&self) -> Range<usize> {
        self.m() + self.n() + 1..self.p() + 1
    }

    pub fn req_id(&self) -> Range<usize> {
        0..self.m()
    }
}

/* --------------------------------------------- -- --------------------------------------------- */
#[derive(Clone, Copy, Debug)]
pub struct HRep {
    pub errc: ReplyCode,
    pub n_req_id: u8,
    pub n_all: u32,
}

impl HRep {
    pub fn m(&self) -> usize {
        self.n_req_id as usize
    }

    pub fn p(&self) -> usize {
        self.n_all as usize
    }

    pub fn payload(&self) -> Range<usize> {
        self.m()..self.p()
    }

    pub fn payload_c(&self) -> Range<usize> {
        self.m()..self.p() + 1
    }

    pub fn req_id(&self) -> Range<usize> {
        0..self.m()
    }
}

/* ------------------------------------------- Helper ------------------------------------------- */
pub fn retrieve_req_id(val: &[u8]) -> IdType {
    #[cfg(not(feature = "id-128bit"))]
    let val = if val.len() > ID_MAX_LEN {
        &val[val.len() - ID_MAX_LEN..]
    } else {
        val
    };

    let offset = ID_MAX_LEN - val.len();
    let value: [u8; ID_MAX_LEN] =
        std::array::from_fn(|i| if i < offset { 0 } else { val[i - offset] });

    IdType::from_be_bytes(value)
}

pub fn store_req_id<'a>(val: IdType, buf: &'a mut [u8; ID_MAX_LEN]) -> &'a [u8] {
    let value = val.to_be_bytes();
    buf.copy_from_slice(&value);

    // remove leading zeros
    let pos = buf.iter().position(|x| *x != 0).unwrap_or(ID_MAX_LEN);
    &buf[pos..]
}

#[test]
fn test_endian() {
    let val = 1 as IdType;
    let buf = val.to_be_bytes();

    assert!(buf[0..ID_MAX_LEN - 1].iter().all(|x| *x == 0));

    let mut buf = [0; ID_MAX_LEN];
    assert!(store_req_id(val, &mut buf).len() == 1);
    assert!(store_req_id(val, &mut buf)[0] == 1);
}

/* ---------------------------------------------------------------------------------------------- */
/*                                           REPLY CODE                                           */
/* ---------------------------------------------------------------------------------------------- */
/// The response code for the request.
#[repr(u8)]
#[derive(Clone, Copy, Debug, Primitive, PartialEq, Eq)]
pub enum ReplyCode {
    /// Successful.
    Okay = 0,

    /// Request is canceled by not handling reply structure.
    ///
    /// Payload likely to be empty.
    Aborted = 1,

    /// There was no route to handle the request.
    ///
    /// Payload may contain the route that was requested.
    NoRoute = 2,

    /// Unkown error.
    Unkown = 4,

    /// User returned error. Payload may contain user defined error context.
    UserError = 100,

    /// User error - invalid data format
    ParseFailed = 101,

    /// User error - invalid argument received
    InvalidArgument = 102,

    /// User error - invalid state
    InvalidState = 103,
}
