# Message Delivery Protocol

- Each field is in **network byte order**.

```plain

    BYTE [0, 4): b0
        bit 30..32) message type (b0 >> 30 & 0b11)
            00: NOTI
            01: REQ
            10: REP
            11: (reserved)
        bit 20..30) n: length of route/method, max 1024 byte allowed. (b0 >> 20 & 0x3ff)
        bit 0 ..20)
            NOTI: (reserved)
            REQ:
                bit 16..20) m: length of request id, max 16 byte allowed, however, 8 byte typically.
            REP:
                bit 16..20) m: length of request id, max 16 byte allowed, however, 8 byte typically.
                bit 8 ..16) e: error code
            
    BYTE [4, 8): p: length of all payload length to read after header. Max 4GB allowed.

    PAYLOAD:
        NOTI: [0..n) route [n) 0 [n+1..p) body
        REQ: [0..m) request_id [m..m+n] route [m+n] 0 [n+m+1..p) body
        REP: [0..m] request id, [m..p) body

```

##  Payload Layout

```
n := length of route
m := length of request id on request / reply message
p := length of total frame, except for the foremost 8-byte headers.
```

- `NOTI`

| `0..` | `n`    | `n+1.. p` |
| ----- | ------ | --------- |
| route | `'\0'` | body      |

- `REQ` 

| `0..m`     | `m.. m+n` | `m+n`  | `m+n+1.. p` |
| ---------- | --------- | ------ | ----------- |
| request_id | route     | `'\0'` | body        |

- `REP`

| `0..m`     | `m.. p` |
| ---------- | ------- |
| request_id | body    |
