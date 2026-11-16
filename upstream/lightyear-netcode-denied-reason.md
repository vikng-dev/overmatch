# The netcode client logs the server's denial reason and discards it

Found in a game using lightyear_netcode. Local draft; not filed.

**Crate:** lightyear_netcode 0.31.0 (crates.io). **Status:** not sent. Source audit: Unchanged on lightyear `main` at 736e1695 (2026-11-05), where the crate now lives under `crates/connection/netcode`; this diff applies there with `patch --dry-run` (MEASURED).

## Defect

On an authenticated `Denied` packet, `Client` (`src/client.rs`) logs `pkt.reason` and moves to
`ClientState::ConnectionDenied`, discarding the reason. The client plugin (`src/client_plugin.rs:187`) then reports
`DisconnectedReason::TransportError(format!("{state:?}"))`, so an application cannot tell a full
server from a ban, an invalid token or a `DeniedReason::Custom` its own `ConnectionRequestHandler`
returned.

## Minimal repro

1. Server with a `ConnectionRequestHandler` returning `Some(DeniedReason::Custom("closed".into()))`.
2. Client connects.
3. Expected: the application can read `Custom("closed")`. Actual: only a log line and
   `TransportError("ConnectionDenied")` (DERIVED from `src/client.rs` and the client plugin).

## Fix

Against the pristine crate (`a/` and `b/` are the crate root):

```diff
--- a/src/client.rs
+++ b/src/client.rs
@@ -11,6 +11,7 @@
     token::{ChallengeToken, ConnectToken},
     utils,
 };
+use lightyear_connection::shared::DeniedReason;
 use lightyear_link::{LinkReceiver, LinkSender, RecvPayload, SendPayload};
 use lightyear_serde::writer::Writer;
 use tracing::{debug, error, info, trace};
@@ -182,6 +183,7 @@
     replay_protection: ReplayProtection,
     should_disconnect: bool,
     should_disconnect_state: ClientState,
+    denied_reason: Option<DeniedReason>,
     send_queue: Vec<SendPayload>,
     packet_queue: Vec<RecvPayload>,
     // We use a Writer (wrapper around BytesMut) here because we will keep re-using the
@@ -223,6 +225,7 @@
             replay_protection: ReplayProtection::new(),
             should_disconnect: false,
             should_disconnect_state: ClientState::Disconnected,
+            denied_reason: None,
             send_queue: Vec::new(),
             packet_queue: Vec::new(),
             writer: Writer::with_capacity(MAX_PKT_BUF_SIZE),
@@ -415,6 +418,7 @@
                     "client connection denied by server. Reason: {:?}",
                     pkt.reason
                 );
+                self.denied_reason = Some(pkt.reason);
                 self.should_disconnect = true;
                 self.should_disconnect_state = ClientState::ConnectionDenied;
                 None
@@ -546,12 +550,20 @@
     /// Returns the netcode client id of the client once it is connected, or returns 0 if not connected.
     pub fn id(&self) -> ClientId {
         self.id
+    }
+
+    /// The authenticated reason the server gave for denying the last connection attempt, if it
+    /// denied it. It survives the client's reset to a terminal state; a new [`connect`](Client::connect)
+    /// clears it.
+    pub fn denied_reason(&self) -> Option<&DeniedReason> {
+        self.denied_reason.as_ref()
     }
 
     /// Prepares the client to connect to the server.
     ///
     /// This function does not perform any IO, it only readies the client to send/receive packets on the next call to [`update`](Client::update).
     pub fn connect(&mut self) {
+        self.denied_reason = None;
         self.reset_connection();
         self.set_state(ClientState::SendingConnectionRequest);
         info!(
```

## Notes

`Client::denied_reason()` keeps the last authenticated reason until the next `connect`. A typed
`DisconnectedReason::Denied(DeniedReason)` in `lightyear_connection` would carry it further, but
changes a public enum; this is the minimal change.
