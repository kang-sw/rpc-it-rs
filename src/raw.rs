use std::ops::Range;

use enum_primitive_derive::Primitive;
use num_traits::FromPrimitive;

/// Every message starts with this header
pub const IDENT: [u8; 4] = [b'+', b'@', b'C', b'|'];

/* ---------------------------------------------------------------------------------------------- */
/*                                       RAW MESSAGE HEADER                                       */
/* ---------------------------------------------------------------------------------------------- */
///
/// # Message Delivery Protocol
///
/// - Each field is in **network byte order**.
///
/// ```plain
///
///     BYTE 0, 1, 2, 3: IDENTIFIER, always "+@C|"
///     BYTE [4, 8): b0
///         bit 29..32) message type
///             000: NOTI
///             001: REQ
///             010: REP
///             011: (reserved)
///             100: (reserved)
///             101: (reserved)
///             110: (reserved)
///             111: (reserved)
///         bit 20..29) n: length of route/method, max 512 byte allowed.
///         bit 0 ..20)
///             NOTI: (reserved)
///             REQ:
///                 bit 16..20) m: length of request id, max 16 byte allowed.
///             REP:
///                 bit 16..20) m: length of request id, max 16 byte allowed.
///                 bit 10..16) e: error code
///             
///     BYTE [8, 12): p: length of all payload length to read. Max 4GB allowed.
///
///     PAYLOAD:
///         NOTI: [0..n) route [n) 0 [n+1..p) payload [p] 0
///         REQ: [0..m) request_id [m..m+n] route [m+n] 0 [n+m+1..p) payload [p] 0
///         REP: [0..m] request id, [m..p) payload [p] 0
///
/// ```
#[repr(C)]
#[derive(Clone, Copy, Debug)]
pub struct RawHead {
    /// Check if this is a valid message
    ident: [u8; 4],

    b0: u32,
    p: u32,
}

pub type RawHeadBuf = [u8; std::mem::size_of::<RawHead>()];

impl RawHead {
    pub fn from_bytes(mut value: RawHeadBuf) -> Option<Self> {
        // Check identifier
        if value[0..4] != IDENT {
            return None;
        }

        // Rearranges frames received in network byte order to system byte order, if necessary.
        const ENDIAN_CHECK: i32 = 1;
        if unsafe { *(ENDIAN_CHECK as *const i8) == 1 } {
            value[4..8].reverse();
            value[8..12].reverse();
        }

        Some(unsafe { std::mem::transmute(value) })
    }

    pub fn to_bytes(&self) -> RawHeadBuf {
        let mut value: RawHeadBuf = unsafe { std::mem::transmute_copy(self) };

        // Rearranges frames received in network byte order to system byte order, if necessary.
        const ENDIAN_CHECK: i32 = 1;
        if unsafe { *(ENDIAN_CHECK as *const i8) == 1 } {
            value[4..8].reverse();
            value[8..12].reverse();
        }

        value
    }

    pub fn new(this: Head) -> Self {
        match this {
            Head::Noti(HNoti { route: n, all_b: p }) => Self {
                ident: IDENT,
                b0: 0 << 29 | (n as u32 & 0x1ff) << 20,
                p,
            },
            Head::Req(HReq {
                route: n,
                req_id: m,
                all_b: p,
            }) => Self {
                ident: IDENT,
                b0: 1 << 29 | (n as u32 & 0x1ff) << 20 | (m as u32 & 0xf) << 16,
                p,
            },
            Head::Rep(HRep {
                errc: e,
                req_id: m,
                all_b: p,
            }) => Self {
                ident: IDENT,
                b0: 2 << 29 | (m as u32 & 0xf) << 16 | (e as u32 & 0x3f) << 10,
                p,
            },
        }
    }

    pub fn parse(&self) -> Option<Head> {
        let t = (self.b0 >> 29) & 0x7;
        let n = (self.b0 >> 20) & 0x1ff;
        let m = (self.b0 >> 16) & 0xf;
        let e = (self.b0 >> 10) & 0x3f;

        match t {
            0 => Some(Head::Noti(HNoti {
                route: n as u16,
                all_b: self.p,
            })),
            1 => Some(Head::Req(HReq {
                route: n as u16,
                req_id: m as u8,
                all_b: self.p,
            })),
            2 => Some(Head::Rep(HRep {
                errc: RepCode::from_u8(e as u8)?,
                req_id: m as u8,
                all_b: self.p,
            })),
            _ => None,
        }
    }
}

/* ---------------------------------------------------------------------------------------------- */
/*                                          PARSED HEADER                                         */
/* ---------------------------------------------------------------------------------------------- */
#[derive(Clone, Copy, Debug)]
pub enum Head {
    Noti(HNoti),
    Req(HReq),
    Rep(HRep),
}

/* --------------------------------------------- -- --------------------------------------------- */
#[derive(Clone, Copy, Debug)]
pub struct HNoti {
    pub route: u16,
    pub all_b: u32,
}

impl HNoti {
    pub fn n(&self) -> usize {
        self.route as usize
    }

    pub fn p(&self) -> usize {
        self.all_b as usize
    }

    pub fn route(&self) -> Range<usize> {
        0..self.n()
    }

    pub fn route_c(&self) -> Range<usize> {
        0..self.n() + 1
    }

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
    pub route: u16,
    pub req_id: u8,
    pub all_b: u32,
}

impl HReq {
    pub fn n(&self) -> usize {
        self.route as usize
    }

    pub fn m(&self) -> usize {
        self.req_id as usize
    }

    pub fn p(&self) -> usize {
        self.all_b as usize
    }

    pub fn route(&self) -> Range<usize> {
        self.m()..self.n()
    }

    pub fn route_c(&self) -> Range<usize> {
        self.m()..self.n() + 1
    }

    pub fn payload(&self) -> Range<usize> {
        self.m() + self.n() + 1..self.p()
    }

    pub fn payload_c(&self) -> Range<usize> {
        self.m() + self.n() + 1..self.p() + 1
    }

    pub fn req_id(&self) -> Range<usize> {
        0..self.m() + 1
    }
}

/* --------------------------------------------- -- --------------------------------------------- */
#[derive(Clone, Copy, Debug)]
pub struct HRep {
    pub errc: RepCode,
    pub req_id: u8,
    pub all_b: u32,
}

impl HRep {
    pub fn m(&self) -> usize {
        self.req_id as usize
    }

    pub fn p(&self) -> usize {
        self.all_b as usize
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
pub fn retrieve_req_id(val: &[u8]) -> u128 {
    debug_assert!(val.len() <= 16);
    let offset = 16 - val.len();
    let value: [u8; 16] = std::array::from_fn(|i| if i < offset { 0 } else { val[i - offset] });
    u128::from_be_bytes(value)
}

pub fn store_req_id<'a>(val: u128, buf: &'a mut [u8; 16]) -> &'a [u8] {
    let value = val.to_be_bytes();
    buf.copy_from_slice(&value);

    // remove leading zeros
    let pos = buf.iter().position(|x| *x != 0).unwrap_or(16);
    &buf[pos..]
}

#[test]
fn test_endian() {
    let val = 1u128;
    let buf = val.to_be_bytes();

    assert!(buf[0..15].iter().all(|x| *x == 0));

    let mut buf: [u8; 16] = [0; 16];
    assert!(store_req_id(val, &mut buf).len() == 1);
    assert!(store_req_id(val, &mut buf)[0] == 1);
}

/* ---------------------------------------------------------------------------------------------- */
/*                                           REPLY CODE                                           */
/* ---------------------------------------------------------------------------------------------- */
/// The response code for the request.
#[repr(u8)]
#[derive(Clone, Copy, Debug, Primitive)]
pub enum RepCode {
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
