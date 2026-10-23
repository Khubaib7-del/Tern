# ADR 0009 · Deleting chats: tombstones, then purge records

| Field | Value |
| --- | --- |
| Status | Accepted |
| Date | 2026-10-24 |
| Decider | Khubaib Nazeer |

## Context

Every chat is a session whose history is an append-only event log (ADR 0004). The store enforces it: a trigger refuses `DELETE` on `events`, so nothing that happened in a run can be rewritten afterwards, and checkpoints, reports, sub-sessions and the Library are all rebuilt from those events.

Users need to delete chats (feedback F12, spec LY-6): a sidebar row could only be archived, there was no Archived view, and nothing could be removed for good. A delete that ignores the trigger would break the guarantee every other part relies on; a delete that only hides the row would keep data the user asked to remove.

## Decision

Deletion has three steps, and the log stays append-only for every chat that has not been purged.

| Step | Store change (schema v5) | Reversible |
| --- | --- | --- |
| **Archive** | `sessions.archived` set (unchanged from v3). The chat leaves the sidebar and shows under **Archived chats** | Yes: Unarchive |
| **Delete** (to the trash) | A tombstone: `sessions.deleted_at` set on the chat and its sub-sessions. They leave the sidebar, tabs, Library, and search, and show under **Archived chats → Trash**. Nothing else changes | Yes: Restore, or Undo for 6 s |
| **Erase / Empty trash** | One row per chat goes into `purges` (session id, title, event count, `deleted_at`, `purged_at`). Only then are the chat's events and session rows deleted, and the files kept beside the database for it (`checkpoints/<id>.json`, `runs/<id>/`) removed | No |

- `purges` is append-only: triggers refuse `UPDATE` and `DELETE` on it, so the record that a chat was erased, and when, cannot be removed.
- The `events_no_delete` trigger is replaced by one that refuses a delete **unless a purge record names the session**. Any other delete still aborts with "the event log is append-only".
- Only chats already in the trash can be erased. Erase and Empty trash ask for confirmation in the desktop app.
- The migration is additive (`ALTER TABLE … ADD COLUMN`, a new table, triggers); existing databases upgrade in place and existing chats keep every event.

## Consequences

- Users can delete chats and get the space back, and a mistaken delete can be undone until the trash is emptied.
- The audit trail keeps one line per erased chat (title and size, not content). A user who wants even the title gone has no way to remove it; this is deliberate and stated here.
- Code that reads events never sees a half-deleted chat: the purge record and the deletes run in one transaction.
- Tests cover the trigger (a delete without a purge record still fails), restore, erase of a chat with sub-sessions, and Empty trash.
