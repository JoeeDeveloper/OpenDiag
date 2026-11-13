# Safety: how writes work

OpenDiag is read-only by default. Two kinds of write exist today, and both go through the same kind of guarded
flow, enforced by the type system rather than by convention.

## Adaptation changes

`opendiag adaptation set` is the only way to change a stored module setting, and it always follows this order.
If any step fails, later steps do not run.

1. **You see the diff** and must type `YES`. There is deliberately **no `--yes` flag**.
2. The confirmation is bound to the exact change shown (channel, old value, new value). Anything else is refused.
3. The current value is **re-read**; if it is no longer the one you were shown, nothing is written.
4. The **backup is written to disk and flushed** (`opendiag-backups.jsonl`, or `--backup-file`, or
   `$OPENDIAG_BACKUPS`). If that fails, nothing is sent to the module.
5. The module is sent a **test**, then the **save** (with the workshop code you supply).
6. The value is **read back**. Only a matching read-back is recorded as `verified`.
7. If the module accepted the save but reads back something else (or the read-back fails), the backup is marked
   `verification_failed` and the error tells you the exact `opendiag backup restore <id>` command. Nothing is
   rewritten automatically, because putting a value back is itself a write that needs your consent.

`opendiag backup list` shows the history. `opendiag backup restore <id>` puts the old value back using the same
guarded flow (so the restore is backed up too and can be undone).

This is enforced by the type system, not just convention: the module-facing `test` and `save` functions require a
`WriteToken` that only the guarded flow can create, so a front end cannot write around it.

The backup file is append-only JSON lines. A corrupt line is an **error** (never skipped), because that file is
what makes a bad change reversible.

### How the guarantee is tested

The central claim is "no write reaches a module unless a durable backup already exists". It is tested as a
property, not just by examples: the virtual modules call a hook the instant a modifying request arrives, and the
hook looks at the backup file *at that moment*. `crates/opendiag-services/tests/write_safety.rs` runs every guarded
flow (clear over KW1281, clear over KWP2000, adaptation over KW1281) 250 times each over a transport that corrupts,
truncates, duplicates and invents frames, and also with an unwritable backup file (where **no** write may arrive).
A separate test proves the checker itself would flag an unguarded write. **Any new write path must be added to
this test.**

This found a real hazard: under corrupted traffic the KW1281 client could acknowledge bytes the module never sent,
and a module could read that noise as a command (blocks have no checksum). The client now does not acknowledge an
invalid block length or an out-of-sequence block counter, and after the first error mid-session it sends nothing
more into that session (reconnect to continue). How a *real* module reacts to a desynchronised byte stream is
unknown; the virtual module assumes it abandons the exchange.

If the write succeeds but updating the backup record afterwards fails (disk full, file removed), the error says the
module **was** changed and appends the bookkeeping failure, rather than reporting only the file error.

### What this does not protect you from

- **A wrong workshop code or channel meaning.** The tool cannot know what a channel does on your module. A
  verified write can still be the wrong value for the car. Only change adaptations you have a documented reason
  to change.
- **A module that fails mid-write.** Power loss during a save is outside what software can guard. Use a healthy
  battery (or a charger) and a stationary vehicle.
- **Anything unverified on hardware.** None of this has run against a real module ([status.md](status.md)). The
  save reply format in particular is unconfirmed, which is why the read-back, not the reply, decides success.

- **Two OpenDiag processes using the same backup file at once.** Ids are assigned by reading the file, so a race
  could reuse an id. One cable talks to one module at a time, so this should not occur; if you script several
  runs, give each its own `--backup-file`.
- **Low-level protocol methods.** `Kw1281Client::clear_fault_codes`, `UdsClient::write_data_by_identifier` and
  similar remain public for tests and tools. Each is gated by `WriteMode`, but any code in the process can unlock
  it. The CLI does not call them; it goes through the guarded flow.
- **A person who has not understood the prompt.** The confirmation proves the exact change was shown and typed
  back, not that it is a good idea.

### Deliberately not implemented

- **Login / security access.** KW1281 has a `Login` block and UDS/KWP have SecurityAccess. OpenDiag only offers
  the pluggable seed-key interface and ships no algorithms. It will not brute-force or bypass logins, immobiliser,
  odometer or tuning protection ([../CLAUDE.md](../CLAUDE.md), ground rule 4).

## Clearing fault codes (same discipline, snapshot instead of restore)

`opendiag dtc clear` erases the stored codes **and their status** (the evidence of what failed and when). That
cannot be undone, and fault codes cannot be written back. So it follows the same flow as adaptation changes, with
a *snapshot* in place of a restore point:

1. You see exactly which codes will be cleared and must type `YES`. There is **no `--yes` flag**.
2. The confirmation is bound to those exact codes (code and status). One for different codes is refused.
3. The codes are **re-read**; if they differ from what you were shown (a new fault appeared, a status changed),
   nothing is cleared, because that evidence would be lost.
4. A **snapshot of the codes is written to disk and flushed first** (same backup file as adaptation changes). If
   that fails, nothing is sent to the module.
5. The clear is sent, using the same crate-private `WriteToken`, so it cannot be called around this flow.
6. The codes are **read again**. Active faults re-register at once, so codes that are still reported are shown
   and recorded (`verification_failed`, with a note listing them), not hidden and not treated as an error.

`opendiag backup list` shows snapshots as "fault codes cleared" with the codes that were stored. A snapshot is a
record, not a restore point: `backup restore` refuses it with an explanation.

The same flow applies to every protocol that can clear codes: UDS, KW1281, KWP2000 on K-line and KWP2000 over
TP2.0. Each protocol only supplies "read the codes" and "send the clear"; the guard around them is shared.
