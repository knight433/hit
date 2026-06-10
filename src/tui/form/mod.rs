//! The request form model: the endpoint's params and body schema flattened
//! into a list of editable rows. The rows ARE the model — serialization
//! walks them back into JSON, and the Shift+X tri-state lives on each row.
//!
//! State semantics (orthogonal `required`/`nullable`, see `model::Field`):
//! - `Filled(v)`  -> sent as `v`
//! - `Empty`      -> blocks submit when required, omitted otherwise
//! - `Null`       -> sent as JSON null (only reachable when nullable)
//! - `Excluded`   -> key omitted entirely (only reachable when optional)

use serde_json::{Map, Value};

use crate::model::{Endpoint, Field, ParamLocation, SchemaNode};

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Section {
    Path,
    Query,
    Header,
    Body,
}

impl Section {
    pub fn label(self) -> &'static str {
        match self {
            Section::Path => "path params",
            Section::Query => "query params",
            Section::Header => "headers",
            Section::Body => "body",
        }
    }
}

#[derive(Debug, Clone, PartialEq)]
pub enum RowKind {
    SectionHeader,
    Scalar,
    Bool,
    Enum(Vec<String>),
    Const,
    /// Any / OneOf / open maps: edited as raw JSON text.
    RawJson,
    ObjectHeader,
    ArrayHeader,
}

#[derive(Debug, Clone, PartialEq)]
pub enum RowState {
    Filled(Value),
    Empty,
    Null,
    Excluded,
}

#[derive(Debug, Clone)]
pub struct FormRow {
    pub section: Section,
    /// Last path segment: field name, or `[i]` for array items. Display only.
    pub label: String,
    /// Indentation inside the body tree (0 for top-level fields and params).
    pub depth: u16,
    pub kind: RowKind,
    pub state: RowState,
    pub required: bool,
    pub nullable: bool,
    pub kind_label: String,
    pub description: Option<String>,
    pub schema: SchemaNode,
    pub collapsed: bool,
    /// Value stashed when cycling away from Filled, restored on re-include.
    saved: Option<Value>,
}

impl FormRow {
    pub fn interactive(&self) -> bool {
        !matches!(self.kind, RowKind::SectionHeader | RowKind::Const)
    }
}

/// Submit-time validation failure.
#[derive(Debug, PartialEq)]
pub struct SubmitError {
    pub row: usize,
    pub message: String,
}

pub struct FormState {
    pub rows: Vec<FormRow>,
    pub cursor: usize,
    /// False when the body is a single non-object root (array/raw JSON).
    body_is_object: bool,
}

impl FormState {
    pub fn new(endpoint: &Endpoint) -> Self {
        let mut rows = Vec::new();
        let mut body_is_object = true;

        for (location, section) in [
            (ParamLocation::Path, Section::Path),
            (ParamLocation::Query, Section::Query),
            (ParamLocation::Header, Section::Header),
        ] {
            let params: Vec<_> = endpoint.params_in(location).collect();
            if params.is_empty() {
                continue;
            }
            rows.push(section_header(section));
            for param in params {
                rows.push(param_row(section, param));
            }
        }

        if let Some(body) = &endpoint.body {
            rows.push(section_header(Section::Body));
            match &body.schema {
                SchemaNode::Object { fields, .. } if !fields.is_empty() => {
                    for field in fields {
                        if field.read_only {
                            continue;
                        }
                        push_field(&mut rows, field, 0, None);
                    }
                }
                other => {
                    // Non-object body: single root row.
                    body_is_object = false;
                    push_node(
                        &mut rows,
                        "body".into(),
                        0,
                        other,
                        body.required,
                        false,
                        None,
                        None,
                        None,
                    );
                }
            }
        }

        let mut form = Self {
            rows,
            cursor: 0,
            body_is_object,
        };
        form.clamp_cursor_to_interactive(1);
        form
    }

    /// Rebuild the body section from a JSON value (after $EDITOR editing).
    pub fn hydrate_body(&mut self, endpoint: &Endpoint, value: &Value) {
        let Some(body) = &endpoint.body else { return };
        // Drop existing body rows.
        if let Some(start) = self
            .rows
            .iter()
            .position(|r| r.section == Section::Body && r.kind == RowKind::SectionHeader)
        {
            self.rows.truncate(start);
        }
        self.rows.push(section_header(Section::Body));
        match &body.schema {
            SchemaNode::Object { fields, .. } if !fields.is_empty() => {
                for field in fields {
                    if field.read_only {
                        continue;
                    }
                    push_field(&mut self.rows, field, 0, Some(value));
                }
            }
            other => {
                self.body_is_object = false;
                push_node(
                    &mut self.rows,
                    "body".into(),
                    0,
                    other,
                    body.required,
                    false,
                    None,
                    None,
                    Some(Some(value.clone())),
                );
            }
        }
        self.clamp_cursor_to_interactive(1);
    }

    // ------------------------------------------------------------ cursor

    /// Rows hidden because an ancestor header is Excluded/Null or collapsed.
    pub fn hidden_mask(&self) -> Vec<bool> {
        let mut mask = vec![false; self.rows.len()];
        let mut i = 0;
        while i < self.rows.len() {
            let row = &self.rows[i];
            let header = matches!(row.kind, RowKind::ObjectHeader | RowKind::ArrayHeader);
            let hide_children = header
                && (row.collapsed
                    || matches!(
                        row.state,
                        RowState::Excluded | RowState::Null | RowState::Empty
                    ));
            if hide_children {
                let end = self.span_end(i);
                for slot in mask.iter_mut().take(end).skip(i + 1) {
                    *slot = true;
                }
                i = end;
            } else {
                i += 1;
            }
        }
        mask
    }

    pub fn move_cursor(&mut self, delta: i64) {
        let hidden = self.hidden_mask();
        let mut i = self.cursor as i64;
        loop {
            i += delta;
            if i < 0 || i >= self.rows.len() as i64 {
                return; // stay put at edges
            }
            let idx = i as usize;
            if self.rows[idx].interactive() && !hidden[idx] {
                self.cursor = idx;
                return;
            }
        }
    }

    fn clamp_cursor_to_interactive(&mut self, direction: i64) {
        let hidden = self.hidden_mask();
        if self
            .rows
            .get(self.cursor)
            .map(|r| r.interactive() && !hidden[self.cursor])
            .unwrap_or(false)
        {
            return;
        }
        self.cursor = 0;
        if !self.rows.is_empty() && (!self.rows[0].interactive() || hidden[0]) {
            self.move_cursor(direction);
        }
    }

    /// Index one past the last descendant of the row at `i`.
    pub fn span_end(&self, i: usize) -> usize {
        let row = &self.rows[i];
        if !matches!(row.kind, RowKind::ObjectHeader | RowKind::ArrayHeader) {
            return i + 1;
        }
        let mut j = i + 1;
        while j < self.rows.len()
            && self.rows[j].section == row.section
            && self.rows[j].kind != RowKind::SectionHeader
            && self.rows[j].depth > row.depth
        {
            j += 1;
        }
        j
    }

    // ---------------------------------------------------- state transitions

    /// Shift+X. Returns a status hint when the cycle is a no-op.
    pub fn cycle_exclusion(&mut self, i: usize) -> Option<&'static str> {
        let row = &mut self.rows[i];
        if !row.interactive() {
            return None;
        }
        // Params have no null on the wire: nullable acts like optional there.
        let nullable = row.nullable && row.section == Section::Body;
        let optional = !row.required;

        let next = match (&row.state, optional, nullable) {
            (RowState::Filled(_) | RowState::Empty, true, true) => RowState::Null,
            (RowState::Null, true, true) => RowState::Excluded,
            (RowState::Excluded, true, true) => restored(row),
            (RowState::Filled(_) | RowState::Empty, true, false) => RowState::Excluded,
            (RowState::Excluded, true, false) => restored(row),
            (RowState::Filled(_) | RowState::Empty, false, true) => RowState::Null,
            (RowState::Null, false, true) => restored(row),
            (_, false, false) => return Some("field is required and not nullable"),
            (state, _, _) => {
                tracing::debug!(?state, "unexpected exclusion-cycle state");
                return None;
            }
        };
        if let RowState::Filled(value) = &row.state {
            row.saved = Some(value.clone());
        }
        row.state = next;
        None
    }

    /// Plain `x`: re-include an Excluded/Null row (terminal-quirk fallback).
    pub fn reinclude(&mut self, i: usize) {
        if matches!(self.rows[i].state, RowState::Excluded | RowState::Null) {
            self.rows[i].state = restored(&self.rows[i]);
        }
    }

    /// Toggle a bool row or cycle an enum row forward.
    pub fn toggle(&mut self, i: usize) {
        let row = &mut self.rows[i];
        match &row.kind {
            RowKind::Bool => {
                let current = matches!(&row.state, RowState::Filled(Value::Bool(true)));
                row.state = RowState::Filled(Value::Bool(!current));
            }
            RowKind::Enum(values) => {
                let current = match &row.state {
                    RowState::Filled(Value::String(s)) => values
                        .iter()
                        .position(|v| v == s)
                        .map(|p| p + 1)
                        .unwrap_or(0),
                    _ => 0,
                };
                let next = values[current % values.len()].clone();
                row.state = RowState::Filled(Value::String(next));
            }
            RowKind::ObjectHeader | RowKind::ArrayHeader => match row.state {
                RowState::Excluded | RowState::Null | RowState::Empty => {
                    row.state = RowState::Filled(Value::Bool(true));
                }
                _ => row.collapsed = !row.collapsed,
            },
            _ => {}
        }
    }

    /// Commit text typed into the inline editor for row `i`.
    pub fn commit_text(&mut self, i: usize, text: &str) -> Result<(), String> {
        let row = &mut self.rows[i];
        if text.is_empty() {
            row.state = RowState::Empty;
            return Ok(());
        }
        let value = match (&row.kind, &row.schema) {
            (RowKind::RawJson, _) => {
                serde_json::from_str(text).map_err(|e| format!("invalid JSON: {e}"))?
            }
            (_, SchemaNode::Integer { .. }) => {
                let n: i64 = text
                    .trim()
                    .parse()
                    .map_err(|_| "not an integer".to_string())?;
                Value::from(n)
            }
            (_, SchemaNode::Number { .. }) => {
                let n: f64 = text
                    .trim()
                    .parse()
                    .map_err(|_| "not a number".to_string())?;
                Value::from(n)
            }
            (_, SchemaNode::Boolean) => {
                let b: bool = text
                    .trim()
                    .parse()
                    .map_err(|_| "true or false".to_string())?;
                Value::Bool(b)
            }
            _ => Value::String(text.to_string()),
        };
        row.state = RowState::Filled(value);
        Ok(())
    }

    /// Current row text for seeding the inline editor.
    pub fn text_of(&self, i: usize) -> String {
        match &self.rows[i].state {
            RowState::Filled(Value::String(s)) => s.clone(),
            RowState::Filled(other) => {
                if self.rows[i].kind == RowKind::RawJson {
                    serde_json::to_string_pretty(other).unwrap_or_default()
                } else {
                    other.to_string()
                }
            }
            _ => String::new(),
        }
    }

    // ------------------------------------------------------------- arrays

    /// Append an item to the array header at `i`.
    pub fn array_append(&mut self, i: usize) {
        let RowKind::ArrayHeader = self.rows[i].kind else {
            return;
        };
        let SchemaNode::Array { item, .. } = self.rows[i].schema.clone() else {
            return;
        };
        let end = self.span_end(i);
        let count = self.direct_children(i).len();
        let depth = self.rows[i].depth + 1;
        let mut new_rows = Vec::new();
        push_array_item(&mut new_rows, &item, count, depth, None);
        self.rows.splice(end..end, new_rows);
        if matches!(
            self.rows[i].state,
            RowState::Empty | RowState::Excluded | RowState::Null
        ) {
            self.rows[i].state = RowState::Filled(Value::Bool(true));
        }
    }

    /// Delete the array item containing row `i` (the row itself if it is an
    /// item root, else its nearest item-root ancestor).
    pub fn array_delete(&mut self, i: usize) {
        let Some((header, item_root)) = self.enclosing_array_item(i) else {
            return;
        };
        let end = self.span_end(item_root);
        self.rows.drain(item_root..end);
        // Renumber labels of the remaining items.
        for (n, idx) in self.direct_children(header).into_iter().enumerate() {
            self.rows[idx].label = format!("[{n}]");
        }
        self.cursor = self.cursor.min(self.rows.len().saturating_sub(1));
        self.clamp_cursor_to_interactive(-1);
    }

    /// Direct child row indices of the header at `i` (depth == header+1).
    fn direct_children(&self, i: usize) -> Vec<usize> {
        let depth = self.rows[i].depth + 1;
        (i + 1..self.span_end(i))
            .filter(|&j| self.rows[j].depth == depth)
            .collect()
    }

    /// For a row inside an array, find (array_header_idx, item_root_idx).
    fn enclosing_array_item(&self, i: usize) -> Option<(usize, usize)> {
        // Walk backwards for the nearest ArrayHeader ancestor.
        let mut best: Option<(usize, usize)> = None;
        for header in (0..=i).rev() {
            if self.rows[header].kind == RowKind::ArrayHeader
                && self.span_end(header) > i
                && self.rows[header].depth < self.rows[i].depth
            {
                let item_root = self
                    .direct_children(header)
                    .into_iter()
                    .take_while(|&j| j <= i)
                    .last()?;
                best = Some((header, item_root));
                break;
            }
        }
        best
    }

    // -------------------------------------------------------- serialization

    /// Validate and serialize all sections.
    pub fn serialize(&self) -> Result<SerializedForm, SubmitError> {
        let mut out = SerializedForm::default();
        let mut i = 0;
        while i < self.rows.len() {
            let row = &self.rows[i];
            match (row.section, &row.kind) {
                (_, RowKind::SectionHeader) => i += 1,
                (Section::Body, _) => {
                    // Body rows: handled as one tree walk from here.
                    let (body, _next) = self.serialize_body_root(i)?;
                    out.body = body;
                    break;
                }
                (section, _) => {
                    if let Some(value) = self.param_value(i)? {
                        let name = row.label.clone();
                        match section {
                            Section::Path => {
                                out.path_params.insert(name, value);
                            }
                            Section::Query => out.query_params.push((name, value)),
                            Section::Header => out.headers.push((name, value)),
                            Section::Body => unreachable!(),
                        }
                    }
                    i += 1;
                }
            }
        }
        Ok(out)
    }

    fn param_value(&self, i: usize) -> Result<Option<String>, SubmitError> {
        let row = &self.rows[i];
        match &row.state {
            RowState::Filled(Value::String(s)) => Ok(Some(s.clone())),
            RowState::Filled(other) => Ok(Some(other.to_string())),
            RowState::Empty if row.required => Err(SubmitError {
                row: i,
                message: format!("required {} '{}' is empty", row.section.label(), row.label),
            }),
            _ => Ok(None),
        }
    }

    /// Body JSON for external editing: like serialize, but `Empty` required
    /// fields become empty-ish placeholders instead of blocking.
    pub fn body_for_editing(&self) -> Value {
        let Some(first) = self.first_body_row() else {
            return Value::Object(Map::new());
        };
        let form = FormState {
            rows: self.clone_rows_lenient(),
            cursor: 0,
            body_is_object: self.body_is_object,
        };
        form.serialize_body_root(first)
            .ok()
            .and_then(|(v, _)| v)
            .unwrap_or(Value::Object(Map::new()))
    }

    fn first_body_row(&self) -> Option<usize> {
        let header = self
            .rows
            .iter()
            .position(|r| r.section == Section::Body && r.kind == RowKind::SectionHeader)?;
        (header + 1 < self.rows.len()).then_some(header + 1)
    }

    /// Copy of the rows with required-Empty leaves filled with type-appropriate
    /// blanks so lenient serialization can't fail.
    fn clone_rows_lenient(&self) -> Vec<FormRow> {
        self.rows
            .iter()
            .map(|row| {
                let mut row = row.clone();
                if row.state == RowState::Empty {
                    row.state = RowState::Filled(blank_value(&row.schema));
                }
                row
            })
            .collect()
    }

    /// Serialize the whole body section starting at its first field row.
    fn serialize_body_root(&self, first: usize) -> Result<(Option<Value>, usize), SubmitError> {
        if !self.body_is_object {
            let (value, next) = self.serialize_node(first)?;
            return Ok((value, next));
        }
        let mut map = Map::new();
        let mut i = first;
        while i < self.rows.len() {
            let row = &self.rows[i];
            if row.kind == RowKind::SectionHeader {
                break;
            }
            if row.depth == 0 {
                let label = row.label.clone();
                let (value, next) = self.serialize_node(i)?;
                if let Some(value) = value {
                    map.insert(label, value);
                }
                i = next;
            } else {
                i += 1;
            }
        }
        Ok((Some(Value::Object(map)), i))
    }

    /// Serialize one row (and its span). Returns (None=omit, value).
    fn serialize_node(&self, i: usize) -> Result<(Option<Value>, usize), SubmitError> {
        let row = &self.rows[i];
        let end = self.span_end(i);
        match &row.state {
            RowState::Excluded => Ok((None, end)),
            RowState::Null => Ok((Some(Value::Null), end)),
            RowState::Empty => {
                if row.required {
                    Err(SubmitError {
                        row: i,
                        message: format!("required field '{}' is empty", row.label),
                    })
                } else {
                    Ok((None, end))
                }
            }
            RowState::Filled(value) => match &row.kind {
                RowKind::ObjectHeader => {
                    let mut map = Map::new();
                    for child in self.direct_children(i) {
                        let label = self.rows[child].label.clone();
                        let (child_value, _) = self.serialize_node(child)?;
                        if let Some(child_value) = child_value {
                            map.insert(label, child_value);
                        }
                    }
                    Ok((Some(Value::Object(map)), end))
                }
                RowKind::ArrayHeader => {
                    let mut items = Vec::new();
                    for child in self.direct_children(i) {
                        let (child_value, _) = self.serialize_node(child)?;
                        if let Some(child_value) = child_value {
                            items.push(child_value);
                        }
                    }
                    Ok((Some(Value::Array(items)), end))
                }
                _ => Ok((Some(value.clone()), end)),
            },
        }
    }
}

#[derive(Debug, Default)]
pub struct SerializedForm {
    pub path_params: std::collections::BTreeMap<String, String>,
    pub query_params: Vec<(String, String)>,
    pub headers: Vec<(String, String)>,
    pub body: Option<Value>,
}

/// A type-appropriate blank for lenient (editor-seed) serialization.
fn blank_value(schema: &SchemaNode) -> Value {
    match schema {
        SchemaNode::String { .. } => Value::String(String::new()),
        SchemaNode::Integer { .. } => Value::from(0),
        SchemaNode::Number { .. } => Value::from(0.0),
        SchemaNode::Boolean => Value::Bool(false),
        SchemaNode::Array { .. } | SchemaNode::Object { .. } => Value::Bool(true), // header marker
        _ => Value::Object(Map::new()),
    }
}

fn restored(row: &FormRow) -> RowState {
    match (&row.saved, &row.kind) {
        (Some(value), _) => RowState::Filled(value.clone()),
        (None, RowKind::ObjectHeader | RowKind::ArrayHeader) => RowState::Filled(Value::Bool(true)),
        (None, _) => RowState::Empty,
    }
}

// ------------------------------------------------------------ row builders

fn section_header(section: Section) -> FormRow {
    FormRow {
        section,
        label: section.label().to_string(),
        depth: 0,
        kind: RowKind::SectionHeader,
        state: RowState::Empty,
        required: false,
        nullable: false,
        kind_label: String::new(),
        description: None,
        schema: SchemaNode::Any,
        collapsed: false,
        saved: None,
    }
}

fn param_row(section: Section, param: &crate::model::Param) -> FormRow {
    let state = match &param.default {
        Some(default) if !default.is_null() => RowState::Filled(default.clone()),
        _ => RowState::Empty,
    };
    FormRow {
        section,
        label: param.name.clone(),
        depth: 0,
        kind: scalar_kind(&param.schema),
        state,
        required: param.required,
        nullable: param.nullable,
        kind_label: param.schema.kind_label(),
        description: param.description.clone(),
        schema: param.schema.clone(),
        collapsed: false,
        saved: None,
    }
}

fn scalar_kind(schema: &SchemaNode) -> RowKind {
    match schema {
        SchemaNode::Boolean => RowKind::Bool,
        SchemaNode::String {
            enum_values: Some(values),
            ..
        } => RowKind::Enum(values.clone()),
        SchemaNode::Integer {
            enum_values: Some(values),
            ..
        } => RowKind::Enum(values.iter().map(|v| v.to_string()).collect()),
        SchemaNode::Const { .. } => RowKind::Const,
        SchemaNode::String { .. } | SchemaNode::Integer { .. } | SchemaNode::Number { .. } => {
            RowKind::Scalar
        }
        SchemaNode::Object { fields, .. } if !fields.is_empty() => RowKind::ObjectHeader,
        SchemaNode::Array { .. } => RowKind::ArrayHeader,
        // Open maps, Any, OneOf: raw JSON editing.
        _ => RowKind::RawJson,
    }
}

/// Push rows for one object field (and its children), optionally hydrating
/// from `parent_value` (the JSON object containing this field).
fn push_field(rows: &mut Vec<FormRow>, field: &Field, depth: u16, parent_value: Option<&Value>) {
    let value = parent_value.map(|v| v.get(&field.name));
    let hydrated = match value {
        // Hydration source present: field missing -> excluded/empty.
        Some(None) => Some(None),
        Some(Some(v)) => Some(Some(v.clone())),
        None => None,
    };
    push_node(
        rows,
        field.name.clone(),
        depth,
        &field.schema,
        field.required,
        field.nullable,
        field.description.clone(),
        field.default.clone(),
        hydrated,
    );
}

/// `hydrated`: None = no hydration (use defaults); Some(None) = hydration
/// says the key is absent; Some(Some(v)) = hydration provides a value.
#[allow(clippy::too_many_arguments)]
fn push_node(
    rows: &mut Vec<FormRow>,
    label: String,
    depth: u16,
    schema: &SchemaNode,
    required: bool,
    nullable: bool,
    description: Option<String>,
    default: Option<Value>,
    hydrated: Option<Option<Value>>,
) {
    let kind = scalar_kind(schema);
    let state = initial_state(&kind, required, &default, &hydrated);
    let header_idx = rows.len();
    rows.push(FormRow {
        section: Section::Body,
        label,
        depth,
        kind: kind.clone(),
        state,
        required,
        nullable,
        kind_label: schema.kind_label(),
        description,
        schema: schema.clone(),
        collapsed: false,
        saved: None,
    });

    let child_value = match &hydrated {
        Some(Some(v)) => Some(v.clone()),
        _ => None,
    };

    match (&kind, schema) {
        (RowKind::ObjectHeader, SchemaNode::Object { fields, .. }) => {
            for field in fields {
                if field.read_only {
                    continue;
                }
                push_field(rows, field, depth + 1, child_value.as_ref());
            }
        }
        (RowKind::ArrayHeader, SchemaNode::Array { item, .. }) => {
            let items: Vec<Value> = child_value
                .as_ref()
                .and_then(|v| v.as_array().cloned())
                .or_else(|| default.as_ref().and_then(|d| d.as_array().cloned()))
                .unwrap_or_default();
            for (idx, item_value) in items.iter().enumerate() {
                push_array_item(rows, item, idx, depth + 1, Some(item_value.clone()));
            }
        }
        (RowKind::Const, SchemaNode::Const { value }) => {
            rows[header_idx].state = RowState::Filled(value.clone());
        }
        _ => {}
    }
}

fn push_array_item(
    rows: &mut Vec<FormRow>,
    item_schema: &SchemaNode,
    index: usize,
    depth: u16,
    value: Option<Value>,
) {
    push_node(
        rows,
        format!("[{index}]"),
        depth,
        item_schema,
        true, // array items are "required" within the array
        false,
        None,
        None,
        value.map(Some),
    );
}

pub(crate) fn initial_state(
    kind: &RowKind,
    required: bool,
    default: &Option<Value>,
    hydrated: &Option<Option<Value>>,
) -> RowState {
    match hydrated {
        Some(None) => {
            return if required {
                RowState::Empty
            } else {
                RowState::Excluded
            };
        }
        Some(Some(Value::Null)) => return RowState::Null,
        Some(Some(value)) => {
            return match kind {
                RowKind::ObjectHeader | RowKind::ArrayHeader => RowState::Filled(Value::Bool(true)),
                _ => RowState::Filled(value.clone()),
            };
        }
        None => {}
    }
    match (kind, default) {
        // Non-null default on a container: materialize it (children rows are
        // generated from the default by the caller).
        (RowKind::ObjectHeader | RowKind::ArrayHeader, Some(d)) if !d.is_null() => {
            RowState::Filled(Value::Bool(true))
        }
        (RowKind::ObjectHeader | RowKind::ArrayHeader, _) => {
            if required {
                RowState::Filled(Value::Bool(true))
            } else {
                RowState::Excluded
            }
        }
        // `Optional[X] = None`: omit by default so server defaults (and PATCH
        // exclude_unset semantics) apply; Shift+X reaches explicit null.
        (_, Some(Value::Null)) => {
            if required {
                RowState::Null
            } else {
                RowState::Excluded
            }
        }
        (_, Some(default)) => RowState::Filled(default.clone()),
        (_, None) => RowState::Empty,
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::spec::build;
    use serde_json::json;

    fn fixture_endpoint(id: &str) -> Endpoint {
        let raw = std::fs::read_to_string(concat!(
            env!("CARGO_MANIFEST_DIR"),
            "/tests/fixtures/fastapi_31.json"
        ))
        .unwrap();
        let doc: Value = serde_json::from_str(&raw).unwrap();
        build(&doc).unwrap().find_endpoint(id).unwrap().clone()
    }

    fn row_index(form: &FormState, label: &str) -> usize {
        form.rows.iter().position(|r| r.label == label).unwrap()
    }

    #[test]
    fn create_user_form_layout_and_initial_states() {
        let form = FormState::new(&fixture_endpoint("create_user_users__post"));
        // required str -> Empty
        assert_eq!(form.rows[row_index(&form, "email")].state, RowState::Empty);
        // enum with default -> Filled(default)
        assert_eq!(
            form.rows[row_index(&form, "role")].state,
            RowState::Filled(json!("member"))
        );
        // Optional[int] = None -> Excluded (omit; server default applies)
        assert_eq!(form.rows[row_index(&form, "age")].state, RowState::Excluded);
        // array with default [] -> materialized empty
        assert_eq!(
            form.rows[row_index(&form, "tags")].state,
            RowState::Filled(json!(true))
        );
        // optional nested object -> Excluded header with hidden children
        let address = row_index(&form, "address");
        assert_eq!(form.rows[address].kind, RowKind::ObjectHeader);
        assert_eq!(form.rows[address].state, RowState::Excluded);
        let hidden = form.hidden_mask();
        assert!(hidden[row_index(&form, "line1")]);
    }

    #[test]
    fn shift_x_cycles_per_required_nullable_matrix() {
        let mut form = FormState::new(&fixture_endpoint("create_user_users__post"));

        // optional + nullable (age, starts Excluded): full cycle
        let age = row_index(&form, "age");
        form.cycle_exclusion(age); // Excluded -> restored (nothing saved -> Empty)
        assert_eq!(form.rows[age].state, RowState::Empty);
        form.commit_text(age, "30").unwrap();
        form.cycle_exclusion(age); // Filled -> Null, saves 30
        assert_eq!(form.rows[age].state, RowState::Null);
        form.cycle_exclusion(age); // Null -> Excluded
        assert_eq!(form.rows[age].state, RowState::Excluded);
        form.cycle_exclusion(age); // Excluded -> restored Filled(30)
        assert_eq!(form.rows[age].state, RowState::Filled(json!(30)));

        // required + nullable (nickname): Filled/Empty <-> Null only
        let nickname = row_index(&form, "nickname");
        form.cycle_exclusion(nickname);
        assert_eq!(form.rows[nickname].state, RowState::Null);
        form.cycle_exclusion(nickname);
        assert_eq!(form.rows[nickname].state, RowState::Empty);

        // required, not nullable (email): no-op with hint
        let email = row_index(&form, "email");
        let hint = form.cycle_exclusion(email);
        assert!(hint.is_some());
        assert_eq!(form.rows[email].state, RowState::Empty);
    }

    #[test]
    fn submit_blocks_on_required_empty_then_serializes() {
        let mut form = FormState::new(&fixture_endpoint("create_user_users__post"));
        let err = form.serialize().unwrap_err();
        assert_eq!(err.row, row_index(&form, "email"));

        form.commit_text(row_index(&form, "email"), "neo@matrix.io")
            .unwrap();
        form.commit_text(row_index(&form, "name"), "Neo").unwrap();
        let nickname = row_index(&form, "nickname");
        form.cycle_exclusion(nickname); // required+nullable -> send null

        let serialized = form.serialize().unwrap();
        let body = serialized.body.unwrap();
        assert_eq!(
            body,
            json!({
                "email": "neo@matrix.io",
                "name": "Neo",
                "nickname": null,
                "role": "member",
                "tags": []
            })
        );
        // address and age Excluded -> keys absent
        assert!(body.get("address").is_none());
        assert!(body.get("age").is_none());
    }

    #[test]
    fn nested_object_include_and_serialize() {
        let mut form = FormState::new(&fixture_endpoint("create_user_users__post"));
        form.commit_text(row_index(&form, "email"), "a@b.c")
            .unwrap();
        form.commit_text(row_index(&form, "name"), "A").unwrap();
        form.cycle_exclusion(row_index(&form, "nickname"));

        let address = row_index(&form, "address");
        form.toggle(address); // re-include
        assert_eq!(form.rows[address].state, RowState::Filled(json!(true)));
        form.commit_text(row_index(&form, "line1"), "1 Main St")
            .unwrap();
        form.commit_text(row_index(&form, "city"), "Zion").unwrap();

        let body = form.serialize().unwrap().body.unwrap();
        assert_eq!(
            body["address"],
            json!({"line1": "1 Main St", "city": "Zion"})
        );
        // line2 optional+nullable, Empty -> omitted
        assert!(body["address"].get("line2").is_none());
    }

    #[test]
    fn array_append_fill_delete() {
        let mut form = FormState::new(&fixture_endpoint("create_item_items__post"));
        form.commit_text(row_index(&form, "sku"), "SKU-1").unwrap();
        form.commit_text(row_index(&form, "price"), "9.5").unwrap();

        let variants = row_index(&form, "variants");
        assert_eq!(form.rows[variants].kind, RowKind::ArrayHeader);
        form.array_append(variants);
        form.array_append(variants);

        // Fill both items: rows for item 0 and 1
        let names: Vec<usize> = form
            .rows
            .iter()
            .enumerate()
            .filter(|(_, r)| r.label == "name" && r.section == Section::Body)
            .map(|(i, _)| i)
            .collect();
        assert_eq!(names.len(), 2);
        let stocks: Vec<usize> = form
            .rows
            .iter()
            .enumerate()
            .filter(|(_, r)| r.label == "stock")
            .map(|(i, _)| i)
            .collect();
        form.commit_text(names[0], "Red").unwrap();
        form.commit_text(stocks[0], "5").unwrap();
        form.commit_text(names[1], "Blue").unwrap();
        form.commit_text(stocks[1], "7").unwrap();

        let body = form.serialize().unwrap().body.unwrap();
        assert_eq!(
            body["variants"],
            json!([{"name": "Red", "stock": 5}, {"name": "Blue", "stock": 7}])
        );
        assert_eq!(body["kind"], json!("physical")); // const auto-filled

        // Delete item 0; item 1 renumbers and survives
        form.array_delete(names[0]);
        let body = form.serialize().unwrap().body.unwrap();
        assert_eq!(body["variants"], json!([{"name": "Blue", "stock": 7}]));
        let item_roots: Vec<&FormRow> = form
            .rows
            .iter()
            .filter(|r| r.label.starts_with('['))
            .collect();
        assert_eq!(item_roots.len(), 1);
        assert_eq!(item_roots[0].label, "[0]");
    }

    #[test]
    fn params_serialize_and_required_path_param_blocks() {
        let mut form = FormState::new(&fixture_endpoint("update_user_users__user_id__patch"));
        let err = form.serialize().unwrap_err();
        assert!(err.message.contains("user_id"));

        let user_id = row_index(&form, "user_id");
        form.commit_text(user_id, "u-42").unwrap();
        let serialized = form.serialize().unwrap();
        assert_eq!(serialized.path_params["user_id"], "u-42");
        // PATCH body: both fields Optional[...] = None -> omitted entirely,
        // which is what exclude_unset-style PATCH endpoints expect.
        assert_eq!(serialized.body.unwrap(), json!({}));
    }

    #[test]
    fn query_param_x_excludes_instead_of_null() {
        let mut form = FormState::new(&fixture_endpoint("list_users_users__get"));
        let limit = row_index(&form, "limit");
        assert_eq!(form.rows[limit].state, RowState::Filled(json!(20)));
        form.cycle_exclusion(limit); // optional, param -> Excluded (never Null)
        assert_eq!(form.rows[limit].state, RowState::Excluded);
        let serialized = form.serialize().unwrap();
        assert!(serialized.query_params.is_empty());
    }

    #[test]
    fn hydrate_body_round_trips_editor_json() {
        let endpoint = fixture_endpoint("create_user_users__post");
        let mut form = FormState::new(&endpoint);
        let edited = json!({
            "email": "trinity@matrix.io",
            "name": "Trinity",
            "nickname": null,
            "address": {"line1": "2 Side St", "city": "Zion"},
            "tags": ["ops", "pilot"]
        });
        form.hydrate_body(&endpoint, &edited);

        // role/age were absent in the edited JSON -> excluded
        assert_eq!(
            form.rows[row_index(&form, "role")].state,
            RowState::Excluded
        );
        let body = form.serialize().unwrap().body.unwrap();
        assert_eq!(body, edited);
    }

    #[test]
    fn enum_toggle_cycles_values() {
        let mut form = FormState::new(&fixture_endpoint("create_user_users__post"));
        let role = row_index(&form, "role");
        form.toggle(role); // member -> viewer
        assert_eq!(form.rows[role].state, RowState::Filled(json!("viewer")));
        form.toggle(role); // viewer -> admin (wraps)
        assert_eq!(form.rows[role].state, RowState::Filled(json!("admin")));
    }

    #[test]
    fn commit_text_validates_types() {
        let mut form = FormState::new(&fixture_endpoint("list_users_users__get"));
        let limit = row_index(&form, "limit");
        assert!(form.commit_text(limit, "abc").is_err());
        assert!(form.commit_text(limit, "50").is_ok());
        assert_eq!(form.rows[limit].state, RowState::Filled(json!(50)));
    }
}
