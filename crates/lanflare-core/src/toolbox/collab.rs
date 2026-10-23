//! Real-time conflict-free (CRDT) collaborative scratchpad and markdown canvas.
//!
//! Sub-phase 10.3: Conflict-free replicated shared scratchpad using an RGA (Replicated Growable Array)
//! sequence CRDT over local gossip and multi-hop mesh. Guarantees Strong Eventual Consistency (SEC),
//! deterministic total ordering via `AtomId` (Lamport timestamp + author fingerprint + session counter),
//! and causality preservation under arbitrary out-of-order packet arrival.

use std::collections::{HashMap, HashSet};
use std::time::SystemTime;

use http_body_util::BodyExt;
use hyper::{Method, Request, Response, StatusCode};
use hyper::body::Incoming;
use lanflare_proto::toolbox::{
    AtomId, ScratchpadAtomDto, ScratchpadEditDto, ScratchpadOpDto, ScratchpadSyncDto,
};
use thiserror::Error;
use tokio::sync::RwLock;

use crate::server::dufs::{empty_box_response, json_response, ServerResponseBody};
use crate::server::ClientConnectionInfo;

/// Error variants encountered during collaborative scratchpad operations.
#[derive(Debug, Error)]
pub enum CollabError {
    #[error("Document '{0}' not found")]
    DocumentNotFound(String),

    #[error("Invalid request payload: {0}")]
    InvalidPayload(String),

    #[error("Serialization error: {0}")]
    Serialization(#[from] serde_json::Error),
}

/// A conflict-free replicated shared scratchpad document.
///
/// Implements an RGA sequence CRDT where each character is modeled as an `Atom`
/// uniquely identified by an `AtomId`. Edits (insertions and deletions) commute
/// and converge to the identical character sequence regardless of arrival order.
#[derive(Clone, Debug)]
pub struct ScratchpadDoc {
    doc_id: String,
    title: String,
    author_fp: String,
    lamport_clock: u64,
    counter: u64,
    atoms: Vec<ScratchpadAtomDto>,
    atom_indices: HashMap<AtomId, usize>,
    pending_atoms: Vec<ScratchpadAtomDto>,
    last_modified_ms: u64,
}

impl ScratchpadDoc {
    /// Creates a new empty collaborative scratchpad document.
    pub fn new(doc_id: impl Into<String>, title: impl Into<String>, author_fp: impl Into<String>) -> Self {
        let now_ms = SystemTime::now()
            .duration_since(SystemTime::UNIX_EPOCH)
            .unwrap_or_default()
            .as_millis() as u64;
        Self {
            doc_id: doc_id.into(),
            title: title.into(),
            author_fp: author_fp.into().to_ascii_uppercase(),
            lamport_clock: 0,
            counter: 0,
            atoms: Vec::new(),
            atom_indices: HashMap::new(),
            pending_atoms: Vec::new(),
            last_modified_ms: now_ms,
        }
    }

    /// Returns the unique document ID.
    pub fn doc_id(&self) -> &str {
        &self.doc_id
    }

    /// Returns the document title.
    pub fn title(&self) -> &str {
        &self.title
    }

    /// Updates the document title.
    pub fn set_title(&mut self, title: impl Into<String>) {
        self.title = title.into();
        self.touch();
    }

    /// Returns the canonical author fingerprint of this document's creator/node.
    pub fn author_fp(&self) -> &str {
        &self.author_fp
    }

    /// Returns the current logical Lamport clock.
    pub fn lamport(&self) -> u64 {
        self.lamport_clock
    }

    /// Returns the timestamp of last modification.
    pub fn last_modified_ms(&self) -> u64 {
        self.last_modified_ms
    }

    /// Computes the visible document text by filtering out deleted atoms (tombstones).
    pub fn text(&self) -> String {
        self.atoms
            .iter()
            .filter(|a| !a.deleted)
            .map(|a| a.value)
            .collect()
    }

    /// Returns the number of visible (non-deleted) characters in the document.
    pub fn len(&self) -> usize {
        self.atoms.iter().filter(|a| !a.deleted).count()
    }

    /// Returns true if the document contains zero visible characters.
    pub fn is_empty(&self) -> bool {
        self.len() == 0
    }

    /// Returns the raw atom sequence (including tombstones).
    pub fn atoms(&self) -> &[ScratchpadAtomDto] {
        &self.atoms
    }

    /// Returns a list of `AtomId`s for visible (non-deleted) characters in sequence order.
    pub fn visible_atom_ids(&self) -> Vec<AtomId> {
        self.atoms
            .iter()
            .filter(|a| !a.deleted)
            .map(|a| a.id.clone())
            .collect()
    }

    /// Finds the `AtomId` corresponding to a visible character index (0-indexed).
    pub fn atom_at_visible_index(&self, visible_idx: usize) -> Option<AtomId> {
        let mut curr = 0;
        for a in &self.atoms {
            if !a.deleted {
                if curr == visible_idx {
                    return Some(a.id.clone());
                }
                curr += 1;
            }
        }
        None
    }

    /// Inserts a character immediately after `after_id` (or at document start if None).
    ///
    /// Increments local Lamport clock and counter, returning the generated `ScratchpadEditDto`.
    pub fn insert_char(&mut self, after_id: Option<AtomId>, ch: char) -> ScratchpadEditDto {
        self.lamport_clock += 1;
        self.counter += 1;
        let now_ms = SystemTime::now()
            .duration_since(SystemTime::UNIX_EPOCH)
            .unwrap_or_default()
            .as_millis() as u64;

        let atom_id = AtomId::new(self.lamport_clock, &self.author_fp, self.counter);
        let atom = ScratchpadAtomDto {
            id: atom_id.clone(),
            after_id: after_id.clone(),
            value: ch,
            deleted: false,
            lamport: self.lamport_clock,
        };

        self.insert_atom_internal(atom);

        ScratchpadEditDto::insert(
            &self.doc_id,
            format!("edit-{}-{}", self.author_fp, self.counter),
            &self.author_fp,
            self.lamport_clock,
            atom_id,
            after_id,
            ch,
            now_ms,
        )
    }

    /// Convenience helper to insert an entire string after `after_id`.
    ///
    /// Returns a vector of generated `ScratchpadEditDto`s in sequential insertion order.
    pub fn insert_str(&mut self, mut after_id: Option<AtomId>, s: &str) -> Vec<ScratchpadEditDto> {
        let mut edits = Vec::with_capacity(s.chars().count());
        for ch in s.chars() {
            let edit = self.insert_char(after_id.clone(), ch);
            if let ScratchpadOpDto::Insert { ref atom_id, .. } = edit.op {
                after_id = Some(atom_id.clone());
            }
            edits.push(edit);
        }
        edits
    }

    /// Deletes the atom identified by `atom_id` (marking it as a tombstone).
    ///
    /// Returns the generated `ScratchpadEditDto` if the atom exists and was not already deleted.
    pub fn delete_atom(&mut self, atom_id: &AtomId) -> Option<ScratchpadEditDto> {
        if let Some(&idx) = self.atom_indices.get(atom_id)
            && !self.atoms[idx].deleted
        {
            self.atoms[idx].deleted = true;
            self.lamport_clock += 1;
            self.touch();

            let now_ms = SystemTime::now()
                .duration_since(SystemTime::UNIX_EPOCH)
                .unwrap_or_default()
                .as_millis() as u64;
            self.counter += 1;

            return Some(ScratchpadEditDto::delete(
                &self.doc_id,
                format!("del-{}-{}", self.author_fp, self.counter),
                &self.author_fp,
                self.lamport_clock,
                atom_id.clone(),
                now_ms,
            ));
        }
        None
    }

    /// Deletes the character at visible index `visible_idx`.
    pub fn delete_char_at(&mut self, visible_idx: usize) -> Option<ScratchpadEditDto> {
        let atom_id = self.atom_at_visible_index(visible_idx)?;
        self.delete_atom(&atom_id)
    }

    /// Applies a remote edit to this document.
    ///
    /// Returns `true` if the edit modified document state.
    pub fn apply_edit(&mut self, edit: &ScratchpadEditDto) -> bool {
        self.lamport_clock = std::cmp::max(self.lamport_clock, edit.lamport);

        match &edit.op {
            ScratchpadOpDto::Insert {
                atom_id,
                after_id,
                value,
            } => {
                let atom = ScratchpadAtomDto {
                    id: atom_id.clone(),
                    after_id: after_id.clone(),
                    value: *value,
                    deleted: false,
                    lamport: edit.lamport,
                };
                self.insert_atom_internal(atom)
            }
            ScratchpadOpDto::Delete { atom_id } => {
                if let Some(&idx) = self.atom_indices.get(atom_id)
                    && !self.atoms[idx].deleted
                {
                    self.atoms[idx].deleted = true;
                    self.touch();
                    return true;
                }
                false
            }
        }
    }

    /// Merges an entire document synchronization snapshot into this document.
    ///
    /// Preserves existing tombstones, topologically orders incoming atoms,
    /// and resolves any previously pending dependencies. Returns the count of newly applied atoms.
    pub fn apply_sync(&mut self, sync: &ScratchpadSyncDto) -> usize {
        self.lamport_clock = std::cmp::max(self.lamport_clock, sync.lamport);
        let mut added = 0;

        for incoming_atom in &sync.atoms {
            if let Some(&existing_idx) = self.atom_indices.get(&incoming_atom.id) {
                // If remote says deleted and local does not, mark deleted
                if incoming_atom.deleted && !self.atoms[existing_idx].deleted {
                    self.atoms[existing_idx].deleted = true;
                    self.touch();
                    added += 1;
                }
            } else if self.insert_atom_internal(incoming_atom.clone()) {
                added += 1;
            }
        }

        self.drain_pending_atoms();
        added
    }

    /// Exports a complete document snapshot for syncing with peers.
    pub fn export_sync(&self) -> ScratchpadSyncDto {
        ScratchpadSyncDto::new(
            &self.doc_id,
            &self.title,
            self.atoms.clone(),
            self.lamport_clock,
            self.last_modified_ms,
        )
    }

    fn touch(&mut self) {
        self.last_modified_ms = SystemTime::now()
            .duration_since(SystemTime::UNIX_EPOCH)
            .unwrap_or_default()
            .as_millis() as u64;
    }

    /// Core RGA insertion routine.
    ///
    /// Deterministically places `atom` using higher-priority sibling skipping and
    /// transitive descendant tracking.
    fn insert_atom_internal(&mut self, atom: ScratchpadAtomDto) -> bool {
        // 1. Idempotency check
        if let Some(&existing_idx) = self.atom_indices.get(&atom.id) {
            if atom.deleted && !self.atoms[existing_idx].deleted {
                self.atoms[existing_idx].deleted = true;
                self.touch();
                return true;
            }
            return false;
        }

        // 2. Check if anchor exists (if after_id is Some)
        let origin_idx = if let Some(ref anchor_id) = atom.after_id {
            match self.atom_indices.get(anchor_id) {
                Some(&pos) => pos as isize,
                None => {
                    // Anchor is not yet present (out-of-order arrival). Buffer it.
                    self.pending_atoms.push(atom);
                    return false;
                }
            }
        } else {
            -1
        };

        // 3. Scan forward from origin_idx to find exact RGA insertion point
        let mut i = (origin_idx + 1) as usize;
        let mut higher_siblings: HashSet<AtomId> = HashSet::new();

        while i < self.atoms.len() {
            let curr = &self.atoms[i];

            if curr.after_id == atom.after_id {
                if curr.id > atom.id {
                    // curr is a sibling with higher priority; skip it
                    higher_siblings.insert(curr.id.clone());
                    i += 1;
                } else {
                    // atom has higher priority than curr; insert ahead of curr
                    break;
                }
            } else if let Some(ref parent) = curr.after_id
                && higher_siblings.contains(parent)
            {
                // curr was inserted after a higher-priority sibling; skip it too
                higher_siblings.insert(curr.id.clone());
                i += 1;
            } else {
                // Encountered an element not belonging to this sibling subtree; break
                break;
            }
        }

        // 4. Insert atom into array
        let atom_id = atom.id.clone();
        self.atoms.insert(i, atom);
        self.touch();

        // 5. Re-index all elements from index i onwards
        for (pos, a) in self.atoms.iter().enumerate().skip(i) {
            self.atom_indices.insert(a.id.clone(), pos);
        }

        // 6. Check if any previously buffered pending atoms can now be resolved
        self.drain_pending_atoms();

        self.atom_indices.contains_key(&atom_id)
    }

    /// Drains any pending atoms whose anchors are now satisfied.
    fn drain_pending_atoms(&mut self) {
        if self.pending_atoms.is_empty() {
            return;
        }

        let mut progress = true;
        while progress && !self.pending_atoms.is_empty() {
            progress = false;
            let mut remaining = Vec::with_capacity(self.pending_atoms.len());

            for atom in std::mem::take(&mut self.pending_atoms) {
                let ready = match &atom.after_id {
                    Some(anchor) => self.atom_indices.contains_key(anchor),
                    None => true,
                };

                if ready {
                    if self.insert_atom_internal(atom) {
                        progress = true;
                    }
                } else {
                    remaining.push(atom);
                }
            }

            self.pending_atoms = remaining;
        }
    }
}

/// Thread-safe registry and manager for collaborative scratchpad documents.
pub struct CollabManager {
    local_fp: String,
    docs: RwLock<HashMap<String, ScratchpadDoc>>,
}

impl CollabManager {
    /// Creates a new `CollabManager` for the specified local node fingerprint.
    pub fn new(local_fp: impl Into<String>) -> Self {
        Self {
            local_fp: local_fp.into().to_ascii_uppercase(),
            docs: RwLock::new(HashMap::new()),
        }
    }

    /// Returns the local node fingerprint.
    pub fn local_fp(&self) -> &str {
        &self.local_fp
    }

    /// Retrieves an existing document or initializes a new one.
    pub async fn get_or_create_doc(&self, doc_id: &str, title: &str) -> ScratchpadDoc {
        let mut docs = self.docs.write().await;
        docs.entry(doc_id.to_string())
            .or_insert_with(|| ScratchpadDoc::new(doc_id, title, &self.local_fp))
            .clone()
    }

    /// Returns the visible text of a document if it exists.
    pub async fn get_doc_text(&self, doc_id: &str) -> Option<String> {
        let docs = self.docs.read().await;
        docs.get(doc_id).map(|d| d.text())
    }

    /// Applies a remote edit to the corresponding document.
    pub async fn apply_remote_edit(&self, edit: ScratchpadEditDto) -> Result<bool, CollabError> {
        let mut docs = self.docs.write().await;
        let doc = docs
            .entry(edit.doc_id.clone())
            .or_insert_with(|| ScratchpadDoc::new(&edit.doc_id, "Untitled", &self.local_fp));
        Ok(doc.apply_edit(&edit))
    }

    /// Merges a remote sync snapshot into the corresponding document.
    pub async fn apply_remote_sync(&self, sync: ScratchpadSyncDto) -> Result<usize, CollabError> {
        let mut docs = self.docs.write().await;
        let doc = docs
            .entry(sync.doc_id.clone())
            .or_insert_with(|| ScratchpadDoc::new(&sync.doc_id, &sync.title, &self.local_fp));
        Ok(doc.apply_sync(&sync))
    }

    /// Exports a document synchronization snapshot.
    pub async fn export_sync(&self, doc_id: &str) -> Option<ScratchpadSyncDto> {
        let docs = self.docs.read().await;
        docs.get(doc_id).map(|d| d.export_sync())
    }

    /// Lists snapshots of all managed scratchpad documents.
    pub async fn list_docs(&self) -> Vec<ScratchpadSyncDto> {
        let docs = self.docs.read().await;
        docs.values().map(|d| d.export_sync()).collect()
    }

    /// Handles incoming Hyper HTTP requests mounted at `/api/lanflare/v1/toolbox/collab/*`.
    pub async fn handle_hyper_request(
        &self,
        req: Request<Incoming>,
        _client: Option<ClientConnectionInfo>,
    ) -> Response<ServerResponseBody> {
        let method = req.method();
        let path = req.uri().path();

        match (method, path) {
            (&Method::POST, "/api/lanflare/v1/toolbox/collab/edit") => {
                let body_bytes = match req.into_body().collect().await {
                    Ok(collected) => collected.to_bytes(),
                    Err(_) => return empty_box_response(StatusCode::BAD_REQUEST),
                };

                let edit: ScratchpadEditDto = match serde_json::from_slice(&body_bytes) {
                    Ok(e) => e,
                    Err(_) => return empty_box_response(StatusCode::UNPROCESSABLE_ENTITY),
                };

                match self.apply_remote_edit(edit).await {
                    Ok(modified) => {
                        let json = serde_json::json!({
                            "status": "applied",
                            "modified": modified
                        });
                        json_response(StatusCode::OK, &json)
                    }
                    Err(_) => empty_box_response(StatusCode::INTERNAL_SERVER_ERROR),
                }
            }

            (&Method::POST, "/api/lanflare/v1/toolbox/collab/sync") => {
                let body_bytes = match req.into_body().collect().await {
                    Ok(collected) => collected.to_bytes(),
                    Err(_) => return empty_box_response(StatusCode::BAD_REQUEST),
                };

                let sync: ScratchpadSyncDto = match serde_json::from_slice(&body_bytes) {
                    Ok(s) => s,
                    Err(_) => return empty_box_response(StatusCode::UNPROCESSABLE_ENTITY),
                };

                let doc_id = sync.doc_id.clone();
                match self.apply_remote_sync(sync).await {
                    Ok(_) => {
                        if let Some(exported) = self.export_sync(&doc_id).await {
                            json_response(StatusCode::OK, &exported)
                        } else {
                            json_response(StatusCode::OK, &serde_json::json!({ "status": "synced" }))
                        }
                    }
                    Err(_) => empty_box_response(StatusCode::INTERNAL_SERVER_ERROR),
                }
            }

            (&Method::GET, "/api/lanflare/v1/toolbox/collab/doc") => {
                let query = req.uri().query().unwrap_or_default();
                let doc_id = query
                    .split('&')
                    .find_map(|p| {
                        let mut parts = p.split('=');
                        if parts.next() == Some("docId") {
                            parts.next()
                        } else {
                            None
                        }
                    })
                    .unwrap_or_default();

                if let Some(doc) = self.export_sync(doc_id).await {
                    json_response(StatusCode::OK, &doc)
                } else {
                    empty_box_response(StatusCode::NOT_FOUND)
                }
            }

            (&Method::GET, "/api/lanflare/v1/toolbox/collab/list") => {
                let docs = self.list_docs().await;
                json_response(StatusCode::OK, &docs)
            }

            _ => empty_box_response(StatusCode::NOT_FOUND),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_crdt_scratchpad_concurrent_convergence() {
        // Verify SEC (Strong Eventual Consistency) under concurrent edits
        let fp_a = "1111111111111111111111111111111111111111111111111111111111111111";
        let fp_b = "2222222222222222222222222222222222222222222222222222222222222222";
        let fp_c = "3333333333333333333333333333333333333333333333333333333333333333";

        let mut doc_a = ScratchpadDoc::new("doc-shared", "Notes.md", fp_a);
        let mut doc_b = ScratchpadDoc::new("doc-shared", "Notes.md", fp_b);
        let mut doc_c = ScratchpadDoc::new("doc-shared", "Notes.md", fp_c);

        // 1. Initial common prefix: Node A types "HELLO"
        let _initial_edits = doc_a.insert_str(None, "HELLO");
        assert_eq!(doc_a.text(), "HELLO");

        // Sync initial state to B and C
        let sync_initial = doc_a.export_sync();
        doc_b.apply_sync(&sync_initial);
        doc_c.apply_sync(&sync_initial);
        assert_eq!(doc_b.text(), "HELLO");
        assert_eq!(doc_c.text(), "HELLO");

        // 2. Concurrent edits at the same anchor point:
        // Find the 'O' atom (visible index 4)
        let o_atom = doc_a.atom_at_visible_index(4).expect("atom for 'O'");

        // Node A inserts " WORLD" after 'O'
        let edits_a = doc_a.insert_str(Some(o_atom.clone()), " WORLD");

        // Node B concurrently inserts " LANFLARE" after 'O'
        let edits_b = doc_b.insert_str(Some(o_atom.clone()), " LANFLARE");

        // Node C concurrently inserts " CRDT" after 'O'
        let edits_c = doc_c.insert_str(Some(o_atom.clone()), " CRDT");

        // 3. Exchange edits in completely DIFFERENT arrival orders:
        // Node A receives B then C
        for edit in &edits_b {
            doc_a.apply_edit(edit);
        }
        for edit in &edits_c {
            doc_a.apply_edit(edit);
        }

        // Node B receives C then A
        for edit in &edits_c {
            doc_b.apply_edit(edit);
        }
        for edit in &edits_a {
            doc_b.apply_edit(edit);
        }

        // Node C receives A then B
        for edit in &edits_a {
            doc_c.apply_edit(edit);
        }
        for edit in &edits_b {
            doc_c.apply_edit(edit);
        }

        // 4. Verify 100% bit-for-bit convergence across all 3 nodes!
        let text_a = doc_a.text();
        let text_b = doc_b.text();
        let text_c = doc_c.text();

        assert_eq!(text_a, text_b, "Doc A and B must converge to identical text");
        assert_eq!(text_b, text_c, "Doc B and C must converge to identical text");

        // 5. Test concurrent delete and insert
        // Node A deletes the first 'H' (index 0)
        let del_edit = doc_a.delete_char_at(0).expect("delete 'H'");
        assert!(!doc_a.text().starts_with('H'));

        // Node B receives the deletion
        doc_b.apply_edit(&del_edit);
        assert_eq!(doc_a.text(), doc_b.text());

        // 6. Test out-of-order arrival and recovery
        // Node A types "!": generates edit with after_id = last atom
        let last_atom = doc_a.atoms().last().unwrap().id.clone();
        let edit_excl = doc_a.insert_char(Some(last_atom), '!');

        // Create fresh Node D that receives edit_excl BEFORE any other atoms
        let mut doc_d = ScratchpadDoc::new("doc-shared", "Notes.md", "4444444444444444444444444444444444444444444444444444444444444444");
        let applied_early = doc_d.apply_edit(&edit_excl);
        assert!(!applied_early, "Anchor missing so edit should be buffered");

        // Now doc_d receives full sync from A
        doc_d.apply_sync(&doc_a.export_sync());
        assert_eq!(doc_d.text(), doc_a.text());
    }

    #[tokio::test]
    async fn test_collab_manager_crud_and_sync() {
        let manager = CollabManager::new("AAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAA");

        let mut doc = manager.get_or_create_doc("scratch-1", "Main Notes").await;
        let edits = doc.insert_str(None, "# LanFlare Collab\n\nShared scratchpad.");
        for edit in edits {
            let _ = manager.apply_remote_edit(edit).await.unwrap();
        }

        let text = manager.get_doc_text("scratch-1").await.unwrap();
        assert_eq!(text, "# LanFlare Collab\n\nShared scratchpad.");

        let snapshot = manager.export_sync("scratch-1").await.unwrap();
        assert_eq!(snapshot.doc_id, "scratch-1");
        assert_eq!(snapshot.title, "Main Notes");
        assert_eq!(snapshot.to_text(), "# LanFlare Collab\n\nShared scratchpad.");

        let docs = manager.list_docs().await;
        assert_eq!(docs.len(), 1);
    }
}
