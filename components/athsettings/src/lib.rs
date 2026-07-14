//! RaeSettings — system settings manager for AthenaOS.
//!
//! Typed, searchable, observable settings with per-user overrides,
//! admin enforcement, cross-device sync, and version migration.
//!
//! See `docs/components/athsettings.md` for the design.
#![cfg_attr(not(test), no_std)]

extern crate alloc;

use alloc::collections::BTreeMap;
use alloc::string::String;
use alloc::vec::Vec;

// ---------------------------------------------------------------------------
// 1. Core types
// ---------------------------------------------------------------------------

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum SettingsError {
    NotFound,
    TypeMismatch,
    ReadOnly,
    AdminRequired,
    ConstraintViolation,
    InvalidKey,
    ImportFailed,
    ExportFailed,
    MigrationFailed,
    SyncConflict,
    AlreadyExists,
}

pub type Result<T> = core::result::Result<T, SettingsError>;

#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub struct SettingId(pub u64);

#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub struct ObserverId(pub u64);

#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub struct UserId(pub u32);

// ---------------------------------------------------------------------------
// 2. Setting value types
// ---------------------------------------------------------------------------

#[derive(Debug, Clone, PartialEq)]
pub enum SettingValue {
    Bool(bool),
    Int(i64),
    Float(u64), // stored as bits to keep Eq
    Str(String),
    Enum(u32),
    Range(i64),
    Color(u32),
    Path(String),
    KeyBinding(String),
    Font(String),
    List(Vec<String>),
}

impl SettingValue {
    pub fn as_bool(&self) -> Option<bool> {
        match self {
            Self::Bool(v) => Some(*v),
            _ => None,
        }
    }

    pub fn as_int(&self) -> Option<i64> {
        match self {
            Self::Int(v) => Some(*v),
            _ => None,
        }
    }

    pub fn as_float_bits(&self) -> Option<u64> {
        match self {
            Self::Float(v) => Some(*v),
            _ => None,
        }
    }

    pub fn as_str(&self) -> Option<&str> {
        match self {
            Self::Str(v) => Some(v),
            Self::Path(v) | Self::KeyBinding(v) | Self::Font(v) => Some(v),
            _ => None,
        }
    }

    pub fn as_enum(&self) -> Option<u32> {
        match self {
            Self::Enum(v) => Some(*v),
            _ => None,
        }
    }

    pub fn as_range(&self) -> Option<i64> {
        match self {
            Self::Range(v) => Some(*v),
            _ => None,
        }
    }

    pub fn as_color(&self) -> Option<u32> {
        match self {
            Self::Color(v) => Some(*v),
            _ => None,
        }
    }

    pub fn type_name(&self) -> &'static str {
        match self {
            Self::Bool(_) => "bool",
            Self::Int(_) => "int",
            Self::Float(_) => "float",
            Self::Str(_) => "string",
            Self::Enum(_) => "enum",
            Self::Range(_) => "range",
            Self::Color(_) => "color",
            Self::Path(_) => "path",
            Self::KeyBinding(_) => "key-binding",
            Self::Font(_) => "font",
            Self::List(_) => "list",
        }
    }

    pub fn same_type(&self, other: &Self) -> bool {
        core::mem::discriminant(self) == core::mem::discriminant(other)
    }
}

// ---------------------------------------------------------------------------
// 3. Setting constraints
// ---------------------------------------------------------------------------

#[derive(Debug, Clone)]
pub enum Constraint {
    IntRange { min: i64, max: i64 },
    FloatRange { min_bits: u64, max_bits: u64 },
    EnumValues(Vec<u32>),
    StringMaxLen(usize),
    PathMustExist,
    RegexPattern(String),
    Custom(String),
}

impl Constraint {
    pub fn validate(&self, value: &SettingValue) -> bool {
        match (self, value) {
            (Self::IntRange { min, max }, SettingValue::Int(v)) => *v >= *min && *v <= *max,
            (Self::IntRange { min, max }, SettingValue::Range(v)) => *v >= *min && *v <= *max,
            (Self::FloatRange { min_bits, max_bits }, SettingValue::Float(v)) => {
                *v >= *min_bits && *v <= *max_bits
            }
            (Self::EnumValues(vals), SettingValue::Enum(v)) => vals.contains(v),
            (Self::StringMaxLen(max), SettingValue::Str(s)) => s.len() <= *max,
            (Self::StringMaxLen(max), SettingValue::Path(s)) => s.len() <= *max,
            _ => true,
        }
    }
}

// ---------------------------------------------------------------------------
// 4. Setting categories
// ---------------------------------------------------------------------------

#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub enum SettingCategory {
    Display,
    Sound,
    Network,
    Bluetooth,
    Accounts,
    Privacy,
    Personalization,
    Apps,
    System,
    Accessibility,
    TimeAndLanguage,
    Gaming,
    Updates,
}

impl SettingCategory {
    pub fn display_name(&self) -> &'static str {
        match self {
            Self::Display => "Display",
            Self::Sound => "Sound",
            Self::Network => "Network & Internet",
            Self::Bluetooth => "Bluetooth & Devices",
            Self::Accounts => "Accounts",
            Self::Privacy => "Privacy & Security",
            Self::Personalization => "Personalization",
            Self::Apps => "Apps",
            Self::System => "System",
            Self::Accessibility => "Accessibility",
            Self::TimeAndLanguage => "Time & Language",
            Self::Gaming => "Gaming",
            Self::Updates => "Updates",
        }
    }

    pub fn icon_name(&self) -> &'static str {
        match self {
            Self::Display => "monitor",
            Self::Sound => "volume",
            Self::Network => "wifi",
            Self::Bluetooth => "bluetooth",
            Self::Accounts => "person",
            Self::Privacy => "shield",
            Self::Personalization => "paintbrush",
            Self::Apps => "grid",
            Self::System => "gear",
            Self::Accessibility => "accessibility",
            Self::TimeAndLanguage => "clock",
            Self::Gaming => "gamepad",
            Self::Updates => "download",
        }
    }

    pub const ALL: &'static [SettingCategory] = &[
        Self::Display,
        Self::Sound,
        Self::Network,
        Self::Bluetooth,
        Self::Accounts,
        Self::Privacy,
        Self::Personalization,
        Self::Apps,
        Self::System,
        Self::Accessibility,
        Self::TimeAndLanguage,
        Self::Gaming,
        Self::Updates,
    ];
}

// ---------------------------------------------------------------------------
// 5. Setting metadata / schema
// ---------------------------------------------------------------------------

#[derive(Debug, Clone)]
pub struct SettingSchema {
    pub id: SettingId,
    pub key: String,
    pub display_name: String,
    pub description: String,
    pub category: SettingCategory,
    pub subcategory: String,
    pub default_value: SettingValue,
    pub constraints: Vec<Constraint>,
    pub requires_restart: bool,
    pub requires_admin: bool,
    pub searchable: bool,
    pub search_keywords: Vec<String>,
    pub version_added: u32,
    pub deprecated: bool,
}

impl SettingSchema {
    pub fn validate(&self, value: &SettingValue) -> Result<()> {
        if !self.default_value.same_type(value) {
            return Err(SettingsError::TypeMismatch);
        }
        for constraint in &self.constraints {
            if !constraint.validate(value) {
                return Err(SettingsError::ConstraintViolation);
            }
        }
        Ok(())
    }
}

pub struct SchemaRegistry {
    pub schemas: BTreeMap<String, SettingSchema>,
    pub by_category: BTreeMap<SettingCategory, Vec<String>>,
    pub next_id: u64,
}

impl SchemaRegistry {
    pub fn new() -> Self {
        Self {
            schemas: BTreeMap::new(),
            by_category: BTreeMap::new(),
            next_id: 1,
        }
    }

    pub fn register(&mut self, mut schema: SettingSchema) -> SettingId {
        let id = SettingId(self.next_id);
        self.next_id += 1;
        schema.id = id;
        let key = schema.key.clone();
        let category = schema.category;
        self.schemas.insert(key.clone(), schema);
        self.by_category
            .entry(category)
            .or_insert_with(Vec::new)
            .push(key);
        id
    }

    pub fn get(&self, key: &str) -> Option<&SettingSchema> {
        self.schemas.get(key)
    }

    pub fn list_category(&self, category: SettingCategory) -> Vec<&SettingSchema> {
        self.by_category
            .get(&category)
            .map(|keys| keys.iter().filter_map(|k| self.schemas.get(k)).collect())
            .unwrap_or_default()
    }

    pub fn all_keys(&self) -> Vec<&str> {
        self.schemas.keys().map(|k| k.as_str()).collect()
    }

    pub fn schema_count(&self) -> usize {
        self.schemas.len()
    }
}

// ---------------------------------------------------------------------------
// 6. Setting storage
// ---------------------------------------------------------------------------

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ValueSource {
    Default,
    SystemOverride,
    AdminEnforced,
    UserSet,
    SyncedFromDevice,
    Imported,
}

#[derive(Debug, Clone)]
pub struct StoredSetting {
    pub key: String,
    pub value: SettingValue,
    pub source: ValueSource,
    pub modified_at: u64,
    pub modified_by: Option<UserId>,
}

pub struct SettingStore {
    pub system_defaults: BTreeMap<String, SettingValue>,
    pub admin_enforced: BTreeMap<String, SettingValue>,
    pub user_values: BTreeMap<UserId, BTreeMap<String, StoredSetting>>,
    pub system_overrides: BTreeMap<String, StoredSetting>,
}

impl SettingStore {
    pub fn new() -> Self {
        Self {
            system_defaults: BTreeMap::new(),
            admin_enforced: BTreeMap::new(),
            user_values: BTreeMap::new(),
            system_overrides: BTreeMap::new(),
        }
    }

    pub fn set_default(&mut self, key: String, value: SettingValue) {
        self.system_defaults.insert(key, value);
    }

    pub fn set_admin_enforced(&mut self, key: String, value: SettingValue) {
        self.admin_enforced.insert(key, value);
    }

    pub fn remove_admin_enforced(&mut self, key: &str) -> bool {
        self.admin_enforced.remove(key).is_some()
    }

    pub fn set_system_override(&mut self, key: String, value: SettingValue, now: u64) {
        self.system_overrides.insert(
            key.clone(),
            StoredSetting {
                key,
                value,
                source: ValueSource::SystemOverride,
                modified_at: now,
                modified_by: None,
            },
        );
    }

    pub fn set_user_value(
        &mut self,
        user: UserId,
        key: String,
        value: SettingValue,
        now: u64,
    ) -> Result<()> {
        if self.admin_enforced.contains_key(&key) {
            return Err(SettingsError::AdminRequired);
        }
        let user_map = self.user_values.entry(user).or_insert_with(BTreeMap::new);
        user_map.insert(
            key.clone(),
            StoredSetting {
                key,
                value,
                source: ValueSource::UserSet,
                modified_at: now,
                modified_by: Some(user),
            },
        );
        Ok(())
    }

    pub fn get_effective(&self, key: &str, user: Option<UserId>) -> Option<&SettingValue> {
        if let Some(enforced) = self.admin_enforced.get(key) {
            return Some(enforced);
        }
        if let Some(uid) = user {
            if let Some(user_map) = self.user_values.get(&uid) {
                if let Some(stored) = user_map.get(key) {
                    return Some(&stored.value);
                }
            }
        }
        if let Some(override_val) = self.system_overrides.get(key) {
            return Some(&override_val.value);
        }
        self.system_defaults.get(key)
    }

    pub fn get_source(&self, key: &str, user: Option<UserId>) -> ValueSource {
        if self.admin_enforced.contains_key(key) {
            return ValueSource::AdminEnforced;
        }
        if let Some(uid) = user {
            if let Some(user_map) = self.user_values.get(&uid) {
                if let Some(stored) = user_map.get(key) {
                    return stored.source;
                }
            }
        }
        if self.system_overrides.contains_key(key) {
            return ValueSource::SystemOverride;
        }
        ValueSource::Default
    }

    pub fn reset_user_value(&mut self, user: UserId, key: &str) -> bool {
        self.user_values
            .get_mut(&user)
            .and_then(|m| m.remove(key))
            .is_some()
    }

    pub fn reset_all_user_values(&mut self, user: UserId) {
        self.user_values.remove(&user);
    }

    pub fn is_admin_enforced(&self, key: &str) -> bool {
        self.admin_enforced.contains_key(key)
    }

    pub fn user_modified_keys(&self, user: UserId) -> Vec<&str> {
        self.user_values
            .get(&user)
            .map(|m| m.keys().map(|k| k.as_str()).collect())
            .unwrap_or_default()
    }

    pub fn admin_enforced_keys(&self) -> Vec<&str> {
        self.admin_enforced.keys().map(|k| k.as_str()).collect()
    }
}

// ---------------------------------------------------------------------------
// 7. Settings search
// ---------------------------------------------------------------------------

pub struct SearchResult {
    pub key: String,
    pub display_name: String,
    pub category: SettingCategory,
    pub relevance: u32,
}

pub struct SettingSearch {
    index: BTreeMap<String, Vec<String>>,
}

impl SettingSearch {
    pub fn new() -> Self {
        Self {
            index: BTreeMap::new(),
        }
    }

    pub fn build_index(&mut self, registry: &SchemaRegistry) {
        self.index.clear();
        for (key, schema) in &registry.schemas {
            if !schema.searchable {
                continue;
            }
            let mut tokens = Vec::new();
            for word in Self::tokenize(&schema.display_name) {
                tokens.push(word);
            }
            for word in Self::tokenize(&schema.description) {
                tokens.push(word);
            }
            for kw in &schema.search_keywords {
                for word in Self::tokenize(kw) {
                    tokens.push(word);
                }
            }
            tokens.push(schema.category.display_name().into());
            for token in tokens {
                self.index
                    .entry(token)
                    .or_insert_with(Vec::new)
                    .push(key.clone());
            }
        }
    }

    fn tokenize(text: &str) -> Vec<String> {
        let mut tokens = Vec::new();
        let mut current = String::new();
        for ch in text.chars() {
            if ch.is_alphanumeric() {
                current.push(if ch.is_uppercase() {
                    (ch as u8 + 32) as char
                } else {
                    ch
                });
            } else if !current.is_empty() {
                tokens.push(core::mem::take(&mut current));
            }
        }
        if !current.is_empty() {
            tokens.push(current);
        }
        tokens
    }

    pub fn search(&self, query: &str, registry: &SchemaRegistry) -> Vec<SearchResult> {
        let query_tokens = Self::tokenize(query);
        if query_tokens.is_empty() {
            return Vec::new();
        }

        let mut scores: BTreeMap<String, u32> = BTreeMap::new();

        for qt in &query_tokens {
            for (token, keys) in &self.index {
                let score = Self::fuzzy_score(qt, token);
                if score > 0 {
                    for key in keys {
                        *scores.entry(key.clone()).or_insert(0) += score;
                    }
                }
            }
        }

        let mut results: Vec<SearchResult> = scores
            .into_iter()
            .filter_map(|(key, relevance)| {
                registry.get(&key).map(|schema| SearchResult {
                    key,
                    display_name: schema.display_name.clone(),
                    category: schema.category,
                    relevance,
                })
            })
            .collect();

        results.sort_by(|a, b| b.relevance.cmp(&a.relevance));
        results
    }

    fn fuzzy_score(query: &str, candidate: &str) -> u32 {
        if candidate == query {
            return 100;
        }
        if candidate.starts_with(query) {
            return 80;
        }
        if candidate.contains(query) {
            return 50;
        }
        let mut qi = query.chars().peekable();
        let mut matched = 0u32;
        for c in candidate.chars() {
            if qi.peek() == Some(&c) {
                qi.next();
                matched += 1;
            }
        }
        if qi.peek().is_none() && matched > 0 {
            matched * 10 / query.len() as u32
        } else {
            0
        }
    }
}

// ---------------------------------------------------------------------------
// 8. Settings observer
// ---------------------------------------------------------------------------

#[derive(Debug, Clone)]
pub struct SettingChangeEvent {
    pub key: String,
    pub old_value: Option<SettingValue>,
    pub new_value: SettingValue,
    pub source: ValueSource,
    pub user: Option<UserId>,
    pub timestamp: u64,
}

pub struct Observer {
    pub id: ObserverId,
    pub watched_keys: Vec<String>,
    pub watched_prefixes: Vec<String>,
    pub watched_categories: Vec<SettingCategory>,
    pub pending_events: Vec<SettingChangeEvent>,
    pub batch_mode: bool,
}

impl Observer {
    pub fn new(id: ObserverId) -> Self {
        Self {
            id,
            watched_keys: Vec::new(),
            watched_prefixes: Vec::new(),
            watched_categories: Vec::new(),
            pending_events: Vec::new(),
            batch_mode: false,
        }
    }

    pub fn watch_key(&mut self, key: String) {
        if !self.watched_keys.contains(&key) {
            self.watched_keys.push(key);
        }
    }

    pub fn watch_prefix(&mut self, prefix: String) {
        if !self.watched_prefixes.contains(&prefix) {
            self.watched_prefixes.push(prefix);
        }
    }

    pub fn watch_category(&mut self, category: SettingCategory) {
        if !self.watched_categories.contains(&category) {
            self.watched_categories.push(category);
        }
    }

    pub fn matches(&self, key: &str, category: SettingCategory) -> bool {
        if self.watched_keys.iter().any(|k| k == key) {
            return true;
        }
        if self
            .watched_prefixes
            .iter()
            .any(|p| key.starts_with(p.as_str()))
        {
            return true;
        }
        self.watched_categories.contains(&category)
    }

    pub fn notify(&mut self, event: SettingChangeEvent) {
        self.pending_events.push(event);
    }

    pub fn drain_events(&mut self) -> Vec<SettingChangeEvent> {
        let mut events = Vec::new();
        core::mem::swap(&mut events, &mut self.pending_events);
        events
    }

    pub fn pending_count(&self) -> usize {
        self.pending_events.len()
    }
}

pub struct ObserverRegistry {
    pub observers: BTreeMap<ObserverId, Observer>,
    pub next_id: u64,
}

impl ObserverRegistry {
    pub fn new() -> Self {
        Self {
            observers: BTreeMap::new(),
            next_id: 1,
        }
    }

    pub fn subscribe(&mut self) -> ObserverId {
        let id = ObserverId(self.next_id);
        self.next_id += 1;
        self.observers.insert(id, Observer::new(id));
        id
    }

    pub fn unsubscribe(&mut self, id: ObserverId) -> bool {
        self.observers.remove(&id).is_some()
    }

    pub fn get_mut(&mut self, id: ObserverId) -> Option<&mut Observer> {
        self.observers.get_mut(&id)
    }

    pub fn notify_all(&mut self, key: &str, category: SettingCategory, event: &SettingChangeEvent) {
        for observer in self.observers.values_mut() {
            if observer.matches(key, category) {
                observer.notify(event.clone());
            }
        }
    }

    pub fn observer_count(&self) -> usize {
        self.observers.len()
    }
}

// ---------------------------------------------------------------------------
// 9. Group policy / admin templates
// ---------------------------------------------------------------------------

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum PolicyAction {
    Enforce,
    Recommend,
    RestrictRange,
    Disable,
}

#[derive(Debug, Clone)]
pub struct PolicyRule {
    pub key: String,
    pub action: PolicyAction,
    pub enforced_value: Option<SettingValue>,
    pub allowed_values: Vec<SettingValue>,
    pub description: String,
    pub enabled: bool,
}

pub struct PolicyEngine {
    pub rules: BTreeMap<String, PolicyRule>,
    pub templates: BTreeMap<String, Vec<String>>,
}

impl PolicyEngine {
    pub fn new() -> Self {
        Self {
            rules: BTreeMap::new(),
            templates: BTreeMap::new(),
        }
    }

    pub fn add_rule(&mut self, rule: PolicyRule) {
        self.rules.insert(rule.key.clone(), rule);
    }

    pub fn remove_rule(&mut self, key: &str) -> bool {
        self.rules.remove(key).is_some()
    }

    pub fn check_allowed(&self, key: &str, value: &SettingValue) -> Result<()> {
        if let Some(rule) = self.rules.get(key) {
            if !rule.enabled {
                return Ok(());
            }
            match rule.action {
                PolicyAction::Enforce => {
                    if let Some(ref enforced) = rule.enforced_value {
                        if value != enforced {
                            return Err(SettingsError::AdminRequired);
                        }
                    }
                }
                PolicyAction::Disable => return Err(SettingsError::ReadOnly),
                PolicyAction::RestrictRange => {
                    if !rule.allowed_values.is_empty() && !rule.allowed_values.contains(value) {
                        return Err(SettingsError::ConstraintViolation);
                    }
                }
                PolicyAction::Recommend => {}
            }
        }
        Ok(())
    }

    pub fn effective_value(&self, key: &str) -> Option<&SettingValue> {
        self.rules.get(key).and_then(|r| {
            if r.enabled && r.action == PolicyAction::Enforce {
                r.enforced_value.as_ref()
            } else {
                None
            }
        })
    }

    pub fn create_template(&mut self, name: String, rule_keys: Vec<String>) {
        self.templates.insert(name, rule_keys);
    }

    pub fn apply_template(&mut self, name: &str) -> Vec<String> {
        let keys = match self.templates.get(name) {
            Some(keys) => keys.clone(),
            None => return Vec::new(),
        };
        let mut applied = Vec::new();
        for key in &keys {
            if let Some(rule) = self.rules.get_mut(key) {
                rule.enabled = true;
                applied.push(key.clone());
            }
        }
        applied
    }

    pub fn list_templates(&self) -> Vec<&str> {
        self.templates.keys().map(|k| k.as_str()).collect()
    }

    pub fn enforced_settings(&self) -> Vec<&str> {
        self.rules
            .iter()
            .filter(|(_, r)| r.enabled && r.action == PolicyAction::Enforce)
            .map(|(k, _)| k.as_str())
            .collect()
    }

    pub fn disabled_settings(&self) -> Vec<&str> {
        self.rules
            .iter()
            .filter(|(_, r)| r.enabled && r.action == PolicyAction::Disable)
            .map(|(k, _)| k.as_str())
            .collect()
    }
}

// ---------------------------------------------------------------------------
// 10. Settings sync
// ---------------------------------------------------------------------------

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum SyncPolicy {
    SyncAll,
    SyncNone,
    SyncSelected,
}

#[derive(Debug, Clone)]
pub struct SyncState {
    pub last_sync: u64,
    pub device_id: [u8; 16],
    pub sync_policy: SyncPolicy,
    pub synced_keys: Vec<String>,
    pub excluded_keys: Vec<String>,
    pub pending_upload: Vec<String>,
    pub pending_download: Vec<String>,
    pub conflict_keys: Vec<String>,
}

impl SyncState {
    pub fn new(device_id: [u8; 16]) -> Self {
        Self {
            last_sync: 0,
            device_id,
            sync_policy: SyncPolicy::SyncAll,
            synced_keys: Vec::new(),
            excluded_keys: Vec::new(),
            pending_upload: Vec::new(),
            pending_download: Vec::new(),
            conflict_keys: Vec::new(),
        }
    }

    pub fn should_sync(&self, key: &str) -> bool {
        match self.sync_policy {
            SyncPolicy::SyncAll => !self.excluded_keys.iter().any(|k| k == key),
            SyncPolicy::SyncNone => false,
            SyncPolicy::SyncSelected => self.synced_keys.iter().any(|k| k == key),
        }
    }

    pub fn mark_changed(&mut self, key: String) {
        if self.should_sync(&key) && !self.pending_upload.contains(&key) {
            self.pending_upload.push(key);
        }
    }

    pub fn mark_synced(&mut self, key: &str, now: u64) {
        self.pending_upload.retain(|k| k != key);
        self.pending_download.retain(|k| k != key);
        self.conflict_keys.retain(|k| k != key);
        self.last_sync = now;
    }

    pub fn add_conflict(&mut self, key: String) {
        if !self.conflict_keys.contains(&key) {
            self.conflict_keys.push(key);
        }
    }

    pub fn has_pending(&self) -> bool {
        !self.pending_upload.is_empty() || !self.pending_download.is_empty()
    }

    pub fn pending_count(&self) -> usize {
        self.pending_upload.len() + self.pending_download.len()
    }

    pub fn conflict_count(&self) -> usize {
        self.conflict_keys.len()
    }
}

// ---------------------------------------------------------------------------
// 11. Import / export
// ---------------------------------------------------------------------------

#[derive(Debug, Clone)]
pub struct SettingsExport {
    pub version: u32,
    pub timestamp: u64,
    pub entries: Vec<(String, SettingValue)>,
    pub checksum: [u8; 32],
}

pub struct ImportExport {
    pub last_export: Option<SettingsExport>,
    pub last_import_timestamp: u64,
    pub import_count: u64,
    pub export_count: u64,
}

impl ImportExport {
    pub fn new() -> Self {
        Self {
            last_export: None,
            last_import_timestamp: 0,
            import_count: 0,
            export_count: 0,
        }
    }

    pub fn export_settings(
        &mut self,
        store: &SettingStore,
        user: Option<UserId>,
        registry: &SchemaRegistry,
        now: u64,
    ) -> SettingsExport {
        let mut entries = Vec::new();
        for key in registry.all_keys() {
            if let Some(value) = store.get_effective(key, user) {
                entries.push((key.into(), value.clone()));
            }
        }
        let export = SettingsExport {
            version: 1,
            timestamp: now,
            entries,
            checksum: [0; 32],
        };
        self.last_export = Some(export.clone());
        self.export_count += 1;
        export
    }

    pub fn import_settings(
        &mut self,
        export: &SettingsExport,
        store: &mut SettingStore,
        user: UserId,
        registry: &SchemaRegistry,
        now: u64,
    ) -> Vec<String> {
        let mut errors = Vec::new();
        for (key, value) in &export.entries {
            if let Some(schema) = registry.get(key) {
                if schema.validate(value).is_ok() {
                    if store
                        .set_user_value(user, key.clone(), value.clone(), now)
                        .is_err()
                    {
                        errors.push(key.clone());
                    }
                } else {
                    errors.push(key.clone());
                }
            }
        }
        self.last_import_timestamp = now;
        self.import_count += 1;
        errors
    }
}

// ---------------------------------------------------------------------------
// 12. Settings migration
// ---------------------------------------------------------------------------

#[derive(Debug, Clone)]
pub struct MigrationStep {
    pub from_version: u32,
    pub to_version: u32,
    pub renames: Vec<(String, String)>,
    pub removals: Vec<String>,
    pub additions: Vec<(String, SettingValue)>,
    pub transforms: Vec<(String, String)>,
}

pub struct MigrationEngine {
    pub steps: Vec<MigrationStep>,
    pub current_version: u32,
}

impl MigrationEngine {
    pub fn new(version: u32) -> Self {
        Self {
            steps: Vec::new(),
            current_version: version,
        }
    }

    pub fn add_step(&mut self, step: MigrationStep) {
        self.steps.push(step);
        self.steps.sort_by_key(|s| s.from_version);
    }

    pub fn needs_migration(&self, from_version: u32) -> bool {
        from_version < self.current_version
    }

    pub fn migrate(
        &self,
        from_version: u32,
        store: &mut SettingStore,
        user: Option<UserId>,
        now: u64,
    ) -> Result<u32> {
        let mut version = from_version;
        for step in &self.steps {
            if step.from_version != version {
                continue;
            }
            if step.to_version > self.current_version {
                break;
            }

            for (old_key, new_key) in &step.renames {
                if let Some(uid) = user {
                    let val = store.get_effective(old_key, Some(uid)).cloned();
                    if let Some(v) = val {
                        let _ = store.set_user_value(uid, new_key.clone(), v, now);
                        store.reset_user_value(uid, old_key);
                    }
                }
            }

            for key in &step.removals {
                if let Some(uid) = user {
                    store.reset_user_value(uid, key);
                }
            }

            for (key, value) in &step.additions {
                store.set_default(key.clone(), value.clone());
            }

            version = step.to_version;
        }

        if version == from_version && from_version != self.current_version {
            return Err(SettingsError::MigrationFailed);
        }
        Ok(version)
    }

    pub fn migration_path(&self, from: u32, to: u32) -> Vec<&MigrationStep> {
        let mut path = Vec::new();
        let mut current = from;
        for step in &self.steps {
            if step.from_version == current && step.to_version <= to {
                path.push(step);
                current = step.to_version;
            }
        }
        path
    }
}

// ---------------------------------------------------------------------------
// 13. Default settings population
// ---------------------------------------------------------------------------

fn register_display_settings(registry: &mut SchemaRegistry) {
    let settings = [
        (
            "display.resolution.width",
            "Resolution Width",
            SettingValue::Int(1920),
        ),
        (
            "display.resolution.height",
            "Resolution Height",
            SettingValue::Int(1080),
        ),
        (
            "display.refresh_rate",
            "Refresh Rate",
            SettingValue::Int(60),
        ),
        ("display.scaling", "Display Scaling", SettingValue::Int(100)),
        ("display.hdr_enabled", "HDR", SettingValue::Bool(false)),
        (
            "display.night_light",
            "Night Light",
            SettingValue::Bool(false),
        ),
        (
            "display.night_light_strength",
            "Night Light Strength",
            SettingValue::Range(50),
        ),
        (
            "display.night_light_schedule",
            "Night Light Schedule",
            SettingValue::Bool(false),
        ),
        (
            "display.color_profile",
            "Color Profile",
            SettingValue::Enum(0),
        ),
        ("display.brightness", "Brightness", SettingValue::Range(80)),
        (
            "display.auto_brightness",
            "Auto Brightness",
            SettingValue::Bool(true),
        ),
    ];
    for (key, name, default) in settings {
        registry.register(SettingSchema {
            id: SettingId(0),
            key: key.into(),
            display_name: name.into(),
            description: String::new(),
            category: SettingCategory::Display,
            subcategory: String::new(),
            default_value: default,
            constraints: Vec::new(),
            requires_restart: false,
            requires_admin: false,
            searchable: true,
            search_keywords: Vec::new(),
            version_added: 1,
            deprecated: false,
        });
    }
}

fn register_sound_settings(registry: &mut SchemaRegistry) {
    let settings = [
        (
            "sound.output_volume",
            "Output Volume",
            SettingValue::Range(80),
        ),
        (
            "sound.input_volume",
            "Input Volume",
            SettingValue::Range(80),
        ),
        (
            "sound.output_device",
            "Output Device",
            SettingValue::Str(String::new()),
        ),
        (
            "sound.input_device",
            "Input Device",
            SettingValue::Str(String::new()),
        ),
        (
            "sound.spatial_audio",
            "Spatial Audio",
            SettingValue::Bool(false),
        ),
        (
            "sound.sound_effects",
            "Sound Effects",
            SettingValue::Bool(true),
        ),
        ("sound.mute", "Mute", SettingValue::Bool(false)),
    ];
    for (key, name, default) in settings {
        registry.register(SettingSchema {
            id: SettingId(0),
            key: key.into(),
            display_name: name.into(),
            description: String::new(),
            category: SettingCategory::Sound,
            subcategory: String::new(),
            default_value: default,
            constraints: Vec::new(),
            requires_restart: false,
            requires_admin: false,
            searchable: true,
            search_keywords: Vec::new(),
            version_added: 1,
            deprecated: false,
        });
    }
}

fn register_network_settings(registry: &mut SchemaRegistry) {
    let settings = [
        ("network.wifi_enabled", "Wi-Fi", SettingValue::Bool(true)),
        (
            "network.airplane_mode",
            "Airplane Mode",
            SettingValue::Bool(false),
        ),
        (
            "network.metered_connection",
            "Metered Connection",
            SettingValue::Bool(false),
        ),
        ("network.proxy_enabled", "Proxy", SettingValue::Bool(false)),
        (
            "network.proxy_address",
            "Proxy Address",
            SettingValue::Str(String::new()),
        ),
        (
            "network.dns_primary",
            "Primary DNS",
            SettingValue::Str(String::new()),
        ),
        (
            "network.dns_secondary",
            "Secondary DNS",
            SettingValue::Str(String::new()),
        ),
        (
            "network.firewall_enabled",
            "Firewall",
            SettingValue::Bool(true),
        ),
        ("network.vpn_enabled", "VPN", SettingValue::Bool(false)),
        (
            "network.hotspot_enabled",
            "Mobile Hotspot",
            SettingValue::Bool(false),
        ),
    ];
    for (key, name, default) in settings {
        registry.register(SettingSchema {
            id: SettingId(0),
            key: key.into(),
            display_name: name.into(),
            description: String::new(),
            category: SettingCategory::Network,
            subcategory: String::new(),
            default_value: default,
            constraints: Vec::new(),
            requires_restart: false,
            requires_admin: false,
            searchable: true,
            search_keywords: Vec::new(),
            version_added: 1,
            deprecated: false,
        });
    }
}

fn register_privacy_settings(registry: &mut SchemaRegistry) {
    let settings = [
        (
            "privacy.location_enabled",
            "Location Services",
            SettingValue::Bool(true),
        ),
        (
            "privacy.camera_enabled",
            "Camera Access",
            SettingValue::Bool(true),
        ),
        (
            "privacy.microphone_enabled",
            "Microphone Access",
            SettingValue::Bool(true),
        ),
        (
            "privacy.diagnostics",
            "Diagnostics Data",
            SettingValue::Enum(0),
        ),
        (
            "privacy.activity_history",
            "Activity History",
            SettingValue::Bool(false),
        ),
        (
            "privacy.advertising_id",
            "Advertising ID",
            SettingValue::Bool(false),
        ),
        (
            "privacy.speech_recognition",
            "Online Speech Recognition",
            SettingValue::Bool(false),
        ),
    ];
    for (key, name, default) in settings {
        registry.register(SettingSchema {
            id: SettingId(0),
            key: key.into(),
            display_name: name.into(),
            description: String::new(),
            category: SettingCategory::Privacy,
            subcategory: String::new(),
            default_value: default,
            constraints: Vec::new(),
            requires_restart: false,
            requires_admin: false,
            searchable: true,
            search_keywords: Vec::new(),
            version_added: 1,
            deprecated: false,
        });
    }
}

fn register_personalization_settings(registry: &mut SchemaRegistry) {
    let settings = [
        ("personalization.theme", "Theme", SettingValue::Enum(0)),
        (
            "personalization.accent_color",
            "Accent Color",
            SettingValue::Color(0x0078D4),
        ),
        (
            "personalization.background_path",
            "Background Image",
            SettingValue::Path(String::new()),
        ),
        (
            "personalization.lock_screen_path",
            "Lock Screen Image",
            SettingValue::Path(String::new()),
        ),
        (
            "personalization.font_family",
            "System Font",
            SettingValue::Font(String::new()),
        ),
        (
            "personalization.font_size",
            "Font Size",
            SettingValue::Int(14),
        ),
        (
            "personalization.taskbar_position",
            "Taskbar Position",
            SettingValue::Enum(0),
        ),
        (
            "personalization.taskbar_auto_hide",
            "Auto-hide Taskbar",
            SettingValue::Bool(false),
        ),
        (
            "personalization.start_menu_layout",
            "Start Menu Layout",
            SettingValue::Enum(0),
        ),
        (
            "personalization.transparency_effects",
            "Transparency Effects",
            SettingValue::Bool(true),
        ),
        (
            "personalization.animation_effects",
            "Animation Effects",
            SettingValue::Bool(true),
        ),
    ];
    for (key, name, default) in settings {
        registry.register(SettingSchema {
            id: SettingId(0),
            key: key.into(),
            display_name: name.into(),
            description: String::new(),
            category: SettingCategory::Personalization,
            subcategory: String::new(),
            default_value: default,
            constraints: Vec::new(),
            requires_restart: false,
            requires_admin: false,
            searchable: true,
            search_keywords: Vec::new(),
            version_added: 1,
            deprecated: false,
        });
    }
}

fn register_system_settings(registry: &mut SchemaRegistry) {
    let settings = [
        (
            "system.clipboard_history",
            "Clipboard History",
            SettingValue::Bool(true),
        ),
        (
            "system.multitasking_snap",
            "Snap Windows",
            SettingValue::Bool(true),
        ),
        (
            "system.remote_desktop",
            "Remote Desktop",
            SettingValue::Bool(false),
        ),
        ("system.power_mode", "Power Mode", SettingValue::Enum(1)),
        (
            "system.sleep_timeout_ac",
            "Sleep Timeout (Plugged In)",
            SettingValue::Int(30),
        ),
        (
            "system.sleep_timeout_battery",
            "Sleep Timeout (Battery)",
            SettingValue::Int(15),
        ),
        (
            "system.screen_timeout_ac",
            "Screen Timeout (Plugged In)",
            SettingValue::Int(10),
        ),
        (
            "system.screen_timeout_battery",
            "Screen Timeout (Battery)",
            SettingValue::Int(5),
        ),
        (
            "system.storage_sense",
            "Storage Sense",
            SettingValue::Bool(true),
        ),
    ];
    for (key, name, default) in settings {
        registry.register(SettingSchema {
            id: SettingId(0),
            key: key.into(),
            display_name: name.into(),
            description: String::new(),
            category: SettingCategory::System,
            subcategory: String::new(),
            default_value: default,
            constraints: Vec::new(),
            requires_restart: false,
            requires_admin: false,
            searchable: true,
            search_keywords: Vec::new(),
            version_added: 1,
            deprecated: false,
        });
    }
}

fn register_accessibility_settings(registry: &mut SchemaRegistry) {
    let settings = [
        (
            "accessibility.high_contrast",
            "High Contrast",
            SettingValue::Bool(false),
        ),
        (
            "accessibility.magnifier",
            "Magnifier",
            SettingValue::Bool(false),
        ),
        (
            "accessibility.magnifier_zoom",
            "Magnifier Zoom Level",
            SettingValue::Int(200),
        ),
        (
            "accessibility.narrator",
            "Narrator",
            SettingValue::Bool(false),
        ),
        (
            "accessibility.closed_captions",
            "Closed Captions",
            SettingValue::Bool(false),
        ),
        (
            "accessibility.sticky_keys",
            "Sticky Keys",
            SettingValue::Bool(false),
        ),
        (
            "accessibility.filter_keys",
            "Filter Keys",
            SettingValue::Bool(false),
        ),
        (
            "accessibility.cursor_size",
            "Cursor Size",
            SettingValue::Int(1),
        ),
        (
            "accessibility.text_cursor_indicator",
            "Text Cursor Indicator",
            SettingValue::Bool(false),
        ),
        (
            "accessibility.color_filters",
            "Color Filters",
            SettingValue::Bool(false),
        ),
    ];
    for (key, name, default) in settings {
        registry.register(SettingSchema {
            id: SettingId(0),
            key: key.into(),
            display_name: name.into(),
            description: String::new(),
            category: SettingCategory::Accessibility,
            subcategory: String::new(),
            default_value: default,
            constraints: Vec::new(),
            requires_restart: false,
            requires_admin: false,
            searchable: true,
            search_keywords: Vec::new(),
            version_added: 1,
            deprecated: false,
        });
    }
}

fn register_time_language_settings(registry: &mut SchemaRegistry) {
    let settings = [
        (
            "time.auto_set",
            "Set Time Automatically",
            SettingValue::Bool(true),
        ),
        (
            "time.timezone_auto",
            "Set Time Zone Automatically",
            SettingValue::Bool(true),
        ),
        (
            "time.24hour_format",
            "24-Hour Format",
            SettingValue::Bool(false),
        ),
        (
            "language.display_language",
            "Display Language",
            SettingValue::Str(String::new()),
        ),
        (
            "language.region",
            "Region",
            SettingValue::Str(String::new()),
        ),
        (
            "language.keyboard_layout",
            "Keyboard Layout",
            SettingValue::Str(String::new()),
        ),
        (
            "language.speech_language",
            "Speech Language",
            SettingValue::Str(String::new()),
        ),
    ];
    for (key, name, default) in settings {
        registry.register(SettingSchema {
            id: SettingId(0),
            key: key.into(),
            display_name: name.into(),
            description: String::new(),
            category: SettingCategory::TimeAndLanguage,
            subcategory: String::new(),
            default_value: default,
            constraints: Vec::new(),
            requires_restart: false,
            requires_admin: false,
            searchable: true,
            search_keywords: Vec::new(),
            version_added: 1,
            deprecated: false,
        });
    }
}

fn register_gaming_settings(registry: &mut SchemaRegistry) {
    let settings = [
        ("gaming.game_mode", "Game Mode", SettingValue::Bool(true)),
        ("gaming.game_bar", "Game Bar", SettingValue::Bool(true)),
        (
            "gaming.captures_enabled",
            "Game Captures",
            SettingValue::Bool(true),
        ),
        (
            "gaming.capture_max_length",
            "Max Recording Length (min)",
            SettingValue::Int(120),
        ),
        (
            "gaming.capture_quality",
            "Capture Quality",
            SettingValue::Enum(1),
        ),
        (
            "gaming.fps_counter",
            "FPS Counter",
            SettingValue::Bool(false),
        ),
    ];
    for (key, name, default) in settings {
        registry.register(SettingSchema {
            id: SettingId(0),
            key: key.into(),
            display_name: name.into(),
            description: String::new(),
            category: SettingCategory::Gaming,
            subcategory: String::new(),
            default_value: default,
            constraints: Vec::new(),
            requires_restart: false,
            requires_admin: false,
            searchable: true,
            search_keywords: Vec::new(),
            version_added: 1,
            deprecated: false,
        });
    }
}

fn register_bluetooth_settings(registry: &mut SchemaRegistry) {
    let settings = [
        ("bluetooth.enabled", "Bluetooth", SettingValue::Bool(true)),
        (
            "bluetooth.discoverable",
            "Discoverable",
            SettingValue::Bool(false),
        ),
        (
            "bluetooth.auto_connect",
            "Auto-connect Paired Devices",
            SettingValue::Bool(true),
        ),
    ];
    for (key, name, default) in settings {
        registry.register(SettingSchema {
            id: SettingId(0),
            key: key.into(),
            display_name: name.into(),
            description: String::new(),
            category: SettingCategory::Bluetooth,
            subcategory: String::new(),
            default_value: default,
            constraints: Vec::new(),
            requires_restart: false,
            requires_admin: false,
            searchable: true,
            search_keywords: Vec::new(),
            version_added: 1,
            deprecated: false,
        });
    }
}

fn register_apps_settings(registry: &mut SchemaRegistry) {
    let settings = [
        (
            "apps.install_source",
            "App Install Source",
            SettingValue::Enum(0),
        ),
        (
            "apps.startup_apps_enabled",
            "Startup Apps",
            SettingValue::Bool(true),
        ),
        (
            "apps.sideloading",
            "Allow Sideloading",
            SettingValue::Bool(true),
        ),
        (
            "apps.default_browser",
            "Default Browser",
            SettingValue::Str(String::new()),
        ),
        (
            "apps.default_email",
            "Default Email Client",
            SettingValue::Str(String::new()),
        ),
        (
            "apps.default_maps",
            "Default Maps",
            SettingValue::Str(String::new()),
        ),
        (
            "apps.default_music",
            "Default Music Player",
            SettingValue::Str(String::new()),
        ),
        (
            "apps.default_photos",
            "Default Photo Viewer",
            SettingValue::Str(String::new()),
        ),
        (
            "apps.default_video",
            "Default Video Player",
            SettingValue::Str(String::new()),
        ),
    ];
    for (key, name, default) in settings {
        registry.register(SettingSchema {
            id: SettingId(0),
            key: key.into(),
            display_name: name.into(),
            description: String::new(),
            category: SettingCategory::Apps,
            subcategory: String::new(),
            default_value: default,
            constraints: Vec::new(),
            requires_restart: false,
            requires_admin: false,
            searchable: true,
            search_keywords: Vec::new(),
            version_added: 1,
            deprecated: false,
        });
    }
}

fn register_accounts_settings(registry: &mut SchemaRegistry) {
    let settings = [
        (
            "accounts.auto_login",
            "Sign-in Automatically",
            SettingValue::Bool(false),
        ),
        (
            "accounts.password_required_wakeup",
            "Password on Wake",
            SettingValue::Bool(true),
        ),
        (
            "accounts.family_safety",
            "Family Safety",
            SettingValue::Bool(false),
        ),
    ];
    for (key, name, default) in settings {
        registry.register(SettingSchema {
            id: SettingId(0),
            key: key.into(),
            display_name: name.into(),
            description: String::new(),
            category: SettingCategory::Accounts,
            subcategory: String::new(),
            default_value: default,
            constraints: Vec::new(),
            requires_restart: false,
            requires_admin: false,
            searchable: true,
            search_keywords: Vec::new(),
            version_added: 1,
            deprecated: false,
        });
    }
}

fn register_update_settings(registry: &mut SchemaRegistry) {
    let settings = [
        (
            "updates.auto_update_policy",
            "Auto-update Policy",
            SettingValue::Enum(1),
        ),
        (
            "updates.insider_program",
            "Insider Program",
            SettingValue::Bool(false),
        ),
        (
            "updates.insider_channel",
            "Insider Channel",
            SettingValue::Enum(0),
        ),
        (
            "updates.active_hours_start",
            "Active Hours Start",
            SettingValue::Int(8),
        ),
        (
            "updates.active_hours_end",
            "Active Hours End",
            SettingValue::Int(22),
        ),
        (
            "updates.defer_feature_days",
            "Defer Feature Updates (days)",
            SettingValue::Int(0),
        ),
        (
            "updates.defer_quality_days",
            "Defer Quality Updates (days)",
            SettingValue::Int(0),
        ),
    ];
    for (key, name, default) in settings {
        registry.register(SettingSchema {
            id: SettingId(0),
            key: key.into(),
            display_name: name.into(),
            description: String::new(),
            category: SettingCategory::Updates,
            subcategory: String::new(),
            default_value: default,
            constraints: Vec::new(),
            requires_restart: false,
            requires_admin: false,
            searchable: true,
            search_keywords: Vec::new(),
            version_added: 1,
            deprecated: false,
        });
    }
}

fn populate_defaults(registry: &mut SchemaRegistry) {
    register_display_settings(registry);
    register_sound_settings(registry);
    register_network_settings(registry);
    register_privacy_settings(registry);
    register_personalization_settings(registry);
    register_system_settings(registry);
    register_accessibility_settings(registry);
    register_time_language_settings(registry);
    register_gaming_settings(registry);
    register_bluetooth_settings(registry);
    register_apps_settings(registry);
    register_accounts_settings(registry);
    register_update_settings(registry);
}

// ---------------------------------------------------------------------------
// 14. Settings manager (top-level)
// ---------------------------------------------------------------------------

pub struct SettingsManager {
    pub registry: SchemaRegistry,
    pub store: SettingStore,
    pub search: SettingSearch,
    pub observers: ObserverRegistry,
    pub policy_engine: PolicyEngine,
    pub sync_state: SyncState,
    pub import_export: ImportExport,
    pub migration: MigrationEngine,
    pub schema_version: u32,
    pub initialized: bool,
}

impl SettingsManager {
    pub fn new() -> Self {
        let mut registry = SchemaRegistry::new();
        populate_defaults(&mut registry);
        let mut store = SettingStore::new();
        for (key, schema) in &registry.schemas {
            store.set_default(key.clone(), schema.default_value.clone());
        }
        let mut search = SettingSearch::new();
        search.build_index(&registry);
        Self {
            registry,
            store,
            search,
            observers: ObserverRegistry::new(),
            policy_engine: PolicyEngine::new(),
            sync_state: SyncState::new([0; 16]),
            import_export: ImportExport::new(),
            migration: MigrationEngine::new(1),
            schema_version: 1,
            initialized: false,
        }
    }

    pub fn get(&self, key: &str, user: Option<UserId>) -> Option<&SettingValue> {
        self.store.get_effective(key, user)
    }

    pub fn get_bool(&self, key: &str, user: Option<UserId>) -> Option<bool> {
        self.get(key, user).and_then(|v| v.as_bool())
    }

    pub fn get_int(&self, key: &str, user: Option<UserId>) -> Option<i64> {
        self.get(key, user).and_then(|v| v.as_int())
    }

    pub fn get_str(&self, key: &str, user: Option<UserId>) -> Option<&str> {
        self.get(key, user).and_then(|v| v.as_str())
    }

    pub fn set(&mut self, key: &str, value: SettingValue, user: UserId, now: u64) -> Result<()> {
        let schema = self.registry.get(key).ok_or(SettingsError::NotFound)?;
        if schema.requires_admin {
            return Err(SettingsError::AdminRequired);
        }
        schema.validate(&value)?;
        self.policy_engine.check_allowed(key, &value)?;

        let old = self.store.get_effective(key, Some(user)).cloned();
        self.store
            .set_user_value(user, key.into(), value.clone(), now)?;

        let category = schema.category;
        let event = SettingChangeEvent {
            key: key.into(),
            old_value: old,
            new_value: value,
            source: ValueSource::UserSet,
            user: Some(user),
            timestamp: now,
        };
        self.observers.notify_all(key, category, &event);
        self.sync_state.mark_changed(key.into());
        Ok(())
    }

    pub fn reset(&mut self, key: &str, user: UserId, now: u64) -> Result<()> {
        if self.store.is_admin_enforced(key) {
            return Err(SettingsError::AdminRequired);
        }
        let schema = self.registry.get(key).ok_or(SettingsError::NotFound)?;
        let old = self.store.get_effective(key, Some(user)).cloned();
        self.store.reset_user_value(user, key);

        let new_val = self
            .store
            .get_effective(key, Some(user))
            .cloned()
            .unwrap_or_else(|| schema.default_value.clone());
        let event = SettingChangeEvent {
            key: key.into(),
            old_value: old,
            new_value: new_val,
            source: ValueSource::Default,
            user: Some(user),
            timestamp: now,
        };
        self.observers.notify_all(key, schema.category, &event);
        Ok(())
    }

    pub fn search_settings(&self, query: &str) -> Vec<SearchResult> {
        self.search.search(query, &self.registry)
    }

    pub fn list_category(&self, category: SettingCategory) -> Vec<&SettingSchema> {
        self.registry.list_category(category)
    }

    pub fn subscribe(&mut self) -> ObserverId {
        self.observers.subscribe()
    }

    pub fn unsubscribe(&mut self, id: ObserverId) -> bool {
        self.observers.unsubscribe(id)
    }

    pub fn export_settings(&mut self, user: Option<UserId>, now: u64) -> SettingsExport {
        self.import_export
            .export_settings(&self.store, user, &self.registry, now)
    }

    pub fn import_settings(
        &mut self,
        export: &SettingsExport,
        user: UserId,
        now: u64,
    ) -> Vec<String> {
        self.import_export
            .import_settings(export, &mut self.store, user, &self.registry, now)
    }

    pub fn setting_count(&self) -> usize {
        self.registry.schema_count()
    }
}

pub static SETTINGS_MANAGER: spin::Mutex<Option<SettingsManager>> = spin::Mutex::new(None);

pub fn init() {
    let mut mgr = SettingsManager::new();
    mgr.initialized = true;
    *SETTINGS_MANAGER.lock() = Some(mgr);
}

#[cfg(test)]
mod tests {
    use super::*;

    fn s(v: &str) -> SettingValue {
        SettingValue::Str(alloc::string::String::from(v))
    }

    #[test]
    fn value_accessors_and_type() {
        assert_eq!(SettingValue::Bool(true).as_bool(), Some(true));
        assert_eq!(SettingValue::Int(42).as_int(), Some(42));
        assert_eq!(SettingValue::Int(42).as_bool(), None); // wrong-type access is safe
        assert_eq!(s("hi").as_str(), Some("hi"));
        assert_eq!(SettingValue::Color(0xFF00FF).as_color(), Some(0xFF00FF));
        assert_eq!(SettingValue::Int(1).type_name(), "int");
        assert!(SettingValue::Int(1).same_type(&SettingValue::Int(9)));
        assert!(!SettingValue::Int(1).same_type(&SettingValue::Bool(false)));
    }

    #[test]
    fn precedence_admin_beats_user_beats_override_beats_default() {
        let user = UserId(7);
        let mut store = SettingStore::new();
        store.set_default("theme".into(), s("light"));
        // default only
        assert_eq!(store.get_effective("theme", Some(user)), Some(&s("light")));
        assert_eq!(store.get_source("theme", Some(user)), ValueSource::Default);
        // system override beats default
        store.set_system_override("theme".into(), s("system-dark"), 1);
        assert_eq!(
            store.get_effective("theme", Some(user)),
            Some(&s("system-dark"))
        );
        assert_eq!(
            store.get_source("theme", Some(user)),
            ValueSource::SystemOverride
        );
        // user value beats system override
        store
            .set_user_value(user, "theme".into(), s("user-blue"), 2)
            .unwrap();
        assert_eq!(
            store.get_effective("theme", Some(user)),
            Some(&s("user-blue"))
        );
        assert_eq!(store.get_source("theme", Some(user)), ValueSource::UserSet);
        // admin-enforced beats everything
        store.set_admin_enforced("theme".into(), s("corp-green"));
        assert_eq!(
            store.get_effective("theme", Some(user)),
            Some(&s("corp-green"))
        );
        assert_eq!(
            store.get_source("theme", Some(user)),
            ValueSource::AdminEnforced
        );
    }

    #[test]
    fn user_cannot_override_admin_enforced_policy() {
        // The security property: a user write to an admin-locked key is REJECTED,
        // and the enforced value still wins.
        let user = UserId(1);
        let mut store = SettingStore::new();
        store.set_default("vpn.required".into(), SettingValue::Bool(true));
        store.set_admin_enforced("vpn.required".into(), SettingValue::Bool(true));
        assert!(store.is_admin_enforced("vpn.required"));
        let res = store.set_user_value(user, "vpn.required".into(), SettingValue::Bool(false), 5);
        assert!(matches!(res, Err(SettingsError::AdminRequired)));
        assert_eq!(
            store.get_effective("vpn.required", Some(user)),
            Some(&SettingValue::Bool(true)) // policy still enforced
        );
    }

    #[test]
    fn lifting_admin_and_resetting_user_restore_lower_layers() {
        let user = UserId(3);
        let mut store = SettingStore::new();
        store.set_default("rate".into(), SettingValue::Int(60));
        store
            .set_user_value(user, "rate".into(), SettingValue::Int(144), 1)
            .unwrap();
        store.set_admin_enforced("rate".into(), SettingValue::Int(30));
        assert_eq!(
            store.get_effective("rate", Some(user)),
            Some(&SettingValue::Int(30))
        );
        // Lift the admin lock → the user value re-emerges.
        assert!(store.remove_admin_enforced("rate"));
        assert_eq!(
            store.get_effective("rate", Some(user)),
            Some(&SettingValue::Int(144))
        );
        // Reset the user value → fall back to default.
        assert!(store.reset_user_value(user, "rate"));
        assert_eq!(
            store.get_effective("rate", Some(user)),
            Some(&SettingValue::Int(60))
        );
        assert!(!store.reset_user_value(user, "rate")); // idempotent: nothing left to reset
    }
}
