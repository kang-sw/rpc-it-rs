use enum_primitive_derive::Primitive;
use num_traits::FromPrimitive;

/// Every message starts with this header
pub const IDENT: [u8; 4] = [b'+', b'@', b'C', b'|'];

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
///         NOTI: [0..n) route [n) null [n+1..p)
///         REQ: [0..n) route [n..m-n) request_id [n+1..p) payload
///         REP: [0..m] request id, [m..p] payload
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

#[derive(Clone, Copy, Debug)]
pub enum Head {
    Noti(HNoti),
    Req(HReq),
    Rep(HRep),
}

#[derive(Clone, Copy, Debug)]
pub struct HNoti {
    pub route: u16,
    pub all_b: u32,
}

#[derive(Clone, Copy, Debug)]
pub struct HReq {
    pub route: u16,
    pub req_id: u8,
    pub all_b: u32,
}

#[derive(Clone, Copy, Debug)]
pub struct HRep {
    pub errc: RepCode,
    pub req_id: u8,
    pub all_b: u32,
}

/// The response code for the request.
#[repr(u8)]
#[derive(Clone, Copy, Debug, Primitive)]
pub enum RepCode {
    /// Successful.
    Okay = 0,

    /// Request is canceled by not handling reply structure.
    Aborted = 1,

    /// There was no route to handle the request.
    NoRoute = 2,

    /// User returned error. Payload may contain user defined error context.
    UserError = 3,

    /// Unkown error.
    Unkown = 4,
}
