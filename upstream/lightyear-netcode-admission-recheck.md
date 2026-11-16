# A challenge issued while the request handler admitted completes after it stops admitting

Found in a game using lightyear_netcode. Local draft; not filed.

**Crate:** lightyear_netcode 0.31.0 (crates.io). **Status:** not sent. Source audit: Unchanged on lightyear `main` at 736e1695 (2026-11-05), where the crate now lives under `crates/connection/netcode`; this diff applies there with `patch --dry-run` (MEASURED).

## Defect

`Server` consults `ConnectionRequestHandler::handle_request` only for the connection request
(`src/server.rs:754-765`). `process_connection_response` rechecks `max_clients` before confirming
(`:822-829`) but not the handler, so a handler that starts denying (admission closed, a ban, a
shutdown) still admits every client whose challenge was issued before.

## Minimal repro

1. Handler backed by an `AtomicBool`, open.
2. Client sends its request and receives a challenge; close the handler before its response
   arrives (delay the client's response packet).
3. Expected: denied with the handler's reason. Actual: connected (DERIVED from `server.rs`).

## Fix

Against the pristine crate (`a/` and `b/` are the crate root):

```diff
--- a/src/server.rs
+++ b/src/server.rs
@@ -826,6 +826,18 @@
                 entity,
             )?;
             return Err(Error::ServerIsFull(id::PeerId::Netcode(id)));
+        }
+
+        // The handler's verdict can change while the challenge is in flight (as the client count
+        // can, checked above). Recheck it before confirming, while the client can still receive an
+        // authenticated denial.
+        if let Some(reason) = self
+            .cfg
+            .connection_request_handler
+            .handle_request(id::PeerId::Netcode(id))
+        {
+            self.send_netcode_packet(DeniedPacket::create(reason), client.send_key, entity)?;
+            return Err(Error::Denied(id::PeerId::Netcode(id)));
         }
 
         let client = self.conn_cache.clients.get_mut(&id).unwrap();
```

## Notes

The recheck sends the handler's reason as an authenticated `Denied` packet, as the `ServerFull`
recheck does. The handler now runs twice per admitted connection, so it must be a query, not a
one-shot reservation (`handle_request` takes `&self`).
