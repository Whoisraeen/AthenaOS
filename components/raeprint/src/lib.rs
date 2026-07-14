//! RaePrint — CUPS-equivalent printing subsystem for AthenaOS.
//!
//! Implements printer discovery, print job lifecycle, queue management,
//! IPP protocol, page description, PPD parsing, print filters, spooling, and
//! PDF generation (the [`pdf`] module — the canonical "produce printable
//! output" path: a print job's text + page setup becomes a spec-valid PDF a
//! printer or "Save as PDF" consumes).
#![cfg_attr(not(test), no_std)]

extern crate alloc;

pub mod pdf;

use alloc::collections::BTreeMap;
use alloc::string::String;
use alloc::vec::Vec;
use core::sync::atomic::{AtomicBool, AtomicU32, AtomicU64, Ordering};

// ===========================================================================
// Printer Discovery
// ===========================================================================

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum DiscoveryProtocol {
    Ipp,
    IppSecure,
    Lpd,
    Usb,
    NetworkBroadcast,
    Mdns,
    Snmp,
    WsdPrint,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum PrinterState {
    Idle,
    Processing,
    Stopped,
    Offline,
    Error,
    Paused,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum MediaSize {
    Letter,
    Legal,
    A4,
    A3,
    A5,
    B5,
    Tabloid,
    Executive,
    Envelope10,
    EnvelopeC5,
    Custom { width_mm: u32, height_mm: u32 },
}

impl MediaSize {
    pub fn width_points(&self) -> u32 {
        match self {
            Self::Letter => 612,
            Self::Legal => 612,
            Self::A4 => 595,
            Self::A3 => 842,
            Self::A5 => 420,
            Self::B5 => 516,
            Self::Tabloid => 792,
            Self::Executive => 522,
            Self::Envelope10 => 297,
            Self::EnvelopeC5 => 459,
            Self::Custom { width_mm, .. } => ((*width_mm as f32) * 2.835) as u32,
        }
    }

    pub fn height_points(&self) -> u32 {
        match self {
            Self::Letter => 792,
            Self::Legal => 1008,
            Self::A4 => 842,
            Self::A3 => 1191,
            Self::A5 => 595,
            Self::B5 => 729,
            Self::Tabloid => 1224,
            Self::Executive => 756,
            Self::Envelope10 => 684,
            Self::EnvelopeC5 => 649,
            Self::Custom { height_mm, .. } => ((*height_mm as f32) * 2.835) as u32,
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ColorMode {
    Monochrome,
    Color,
    AutoDetect,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum DuplexMode {
    None,
    LongEdge,
    ShortEdge,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum PrintQuality {
    Draft,
    Normal,
    High,
    Photo,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Resolution {
    pub x_dpi: u32,
    pub y_dpi: u32,
}

#[derive(Debug, Clone)]
pub struct PrinterCapabilities {
    pub color: bool,
    pub duplex: bool,
    pub staple: bool,
    pub punch: bool,
    pub collate: bool,
    pub copies_max: u32,
    pub resolutions: Vec<Resolution>,
    pub supported_media: Vec<MediaSize>,
    pub trays: Vec<String>,
    pub max_pages_per_minute: u32,
}

#[derive(Debug, Clone)]
pub struct PrinterInfo {
    pub name: String,
    pub uri: String,
    pub make: String,
    pub model: String,
    pub serial: Option<String>,
    pub state: PrinterState,
    pub capabilities: PrinterCapabilities,
    pub default_media: MediaSize,
    pub location: Option<String>,
    pub info_text: Option<String>,
    pub discovery_protocol: DiscoveryProtocol,
    pub accepting_jobs: bool,
    pub shared: bool,
    pub job_count: u32,
}

pub struct PrinterDiscovery {
    discovered: Vec<PrinterInfo>,
    scan_active: AtomicBool,
    scan_count: AtomicU32,
}

impl PrinterDiscovery {
    pub fn new() -> Self {
        Self {
            discovered: Vec::new(),
            scan_active: AtomicBool::new(false),
            scan_count: AtomicU32::new(0),
        }
    }

    pub fn start_scan(&self, protocols: &[DiscoveryProtocol]) {
        self.scan_active.store(true, Ordering::SeqCst);
        let _ = protocols;
    }

    pub fn stop_scan(&self) {
        self.scan_active.store(false, Ordering::SeqCst);
        self.scan_count.fetch_add(1, Ordering::Relaxed);
    }

    pub fn add_discovered(&mut self, printer: PrinterInfo) {
        self.discovered.push(printer);
    }

    pub fn get_discovered(&self) -> &[PrinterInfo] {
        &self.discovered
    }

    pub fn find_by_name(&self, name: &str) -> Option<&PrinterInfo> {
        self.discovered.iter().find(|p| p.name == name)
    }

    pub fn find_by_uri(&self, uri: &str) -> Option<&PrinterInfo> {
        self.discovered.iter().find(|p| p.uri == uri)
    }

    pub fn clear(&mut self) {
        self.discovered.clear();
    }
}

// ===========================================================================
// Print Job Lifecycle
// ===========================================================================

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum JobState {
    Pending,
    PendingHeld,
    Processing,
    ProcessingStopped,
    Canceled,
    Aborted,
    Completed,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Orientation {
    Portrait,
    Landscape,
    ReversePortrait,
    ReverseLandscape,
}

#[derive(Debug, Clone)]
pub struct PageRange {
    pub start: u32,
    pub end: u32,
}

#[derive(Debug, Clone)]
pub struct PrintDocument {
    pub name: String,
    pub mime_type: String,
    pub data: Vec<u8>,
    pub total_pages: Option<u32>,
}

#[derive(Debug, Clone)]
pub struct PrintJob {
    pub id: u32,
    pub printer_name: String,
    pub document: PrintDocument,
    pub copies: u32,
    pub media: MediaSize,
    pub orientation: Orientation,
    pub quality: PrintQuality,
    pub color_mode: ColorMode,
    pub duplex: DuplexMode,
    pub page_ranges: Option<Vec<PageRange>>,
    pub priority: u32,
    pub state: JobState,
    pub submitted_at: u64,
    pub started_at: Option<u64>,
    pub completed_at: Option<u64>,
    pub pages_printed: u32,
    pub owner: String,
    pub job_name: String,
    pub collate: bool,
    pub reverse_order: bool,
    pub n_up: u32,
    pub fit_to_page: bool,
}

impl PrintJob {
    pub fn new(id: u32, printer_name: String, document: PrintDocument, owner: String) -> Self {
        Self {
            id,
            printer_name,
            document,
            copies: 1,
            media: MediaSize::Letter,
            orientation: Orientation::Portrait,
            quality: PrintQuality::Normal,
            color_mode: ColorMode::AutoDetect,
            duplex: DuplexMode::None,
            page_ranges: None,
            priority: 50,
            state: JobState::Pending,
            submitted_at: 0,
            started_at: None,
            completed_at: None,
            pages_printed: 0,
            owner,
            job_name: String::new(),
            collate: true,
            reverse_order: false,
            n_up: 1,
            fit_to_page: false,
        }
    }

    pub fn hold(&mut self) {
        if self.state == JobState::Pending {
            self.state = JobState::PendingHeld;
        }
    }

    pub fn release(&mut self) {
        if self.state == JobState::PendingHeld {
            self.state = JobState::Pending;
        }
    }

    pub fn cancel(&mut self) {
        match self.state {
            JobState::Pending | JobState::PendingHeld | JobState::Processing => {
                self.state = JobState::Canceled;
            }
            _ => {}
        }
    }

    pub fn start_processing(&mut self, timestamp: u64) {
        self.state = JobState::Processing;
        self.started_at = Some(timestamp);
    }

    pub fn complete(&mut self, timestamp: u64) {
        self.state = JobState::Completed;
        self.completed_at = Some(timestamp);
    }

    pub fn abort(&mut self) {
        self.state = JobState::Aborted;
    }

    pub fn total_sheets_needed(&self) -> u32 {
        let pages = self.effective_page_count();
        let sheets_per_copy = if self.duplex != DuplexMode::None {
            (pages + 1) / 2
        } else {
            pages
        };
        sheets_per_copy * self.copies
    }

    fn effective_page_count(&self) -> u32 {
        if let Some(ref ranges) = self.page_ranges {
            ranges.iter().map(|r| r.end - r.start + 1).sum()
        } else {
            self.document.total_pages.unwrap_or(1)
        }
    }
}

// ===========================================================================
// Print Queue — per-printer job scheduling
// ===========================================================================

pub struct PrintQueue {
    pub printer_name: String,
    jobs: Vec<PrintJob>,
    paused: bool,
    max_jobs: usize,
    next_job_id: AtomicU32,
}

impl PrintQueue {
    pub fn new(printer_name: String) -> Self {
        Self {
            printer_name,
            jobs: Vec::new(),
            paused: false,
            max_jobs: 1000,
            next_job_id: AtomicU32::new(1),
        }
    }

    /// Submit a job into the queue, assigning it a fresh id. Returns `None` if
    /// the queue is at `max_jobs` capacity (back-pressure rather than unbounded
    /// growth); otherwise the new job id.
    pub fn try_submit_job(&mut self, mut job: PrintJob) -> Option<u32> {
        if self.jobs.len() >= self.max_jobs {
            return None;
        }
        let id = self.next_job_id.fetch_add(1, Ordering::Relaxed);
        job.id = id;
        job.printer_name = self.printer_name.clone();
        self.insert_by_priority(job);
        Some(id)
    }

    /// Submit a job, assigning it a fresh id and returning it. Jobs over the
    /// `max_jobs` cap are still accepted by this convenience wrapper (the cap is
    /// enforced by [`Self::try_submit_job`]); kept for existing callers.
    pub fn submit_job(&mut self, mut job: PrintJob) -> u32 {
        let id = self.next_job_id.fetch_add(1, Ordering::Relaxed);
        job.id = id;
        job.printer_name = self.printer_name.clone();
        self.insert_by_priority(job);
        id
    }

    /// The configured maximum number of jobs this queue will hold.
    pub fn max_jobs(&self) -> usize {
        self.max_jobs
    }

    fn insert_by_priority(&mut self, job: PrintJob) {
        let pos = self.jobs.iter().position(|j| j.priority < job.priority);
        match pos {
            Some(idx) => self.jobs.insert(idx, job),
            None => self.jobs.push(job),
        }
    }

    pub fn next_job(&mut self) -> Option<&mut PrintJob> {
        if self.paused {
            return None;
        }
        self.jobs.iter_mut().find(|j| j.state == JobState::Pending)
    }

    pub fn get_job(&self, id: u32) -> Option<&PrintJob> {
        self.jobs.iter().find(|j| j.id == id)
    }

    pub fn get_job_mut(&mut self, id: u32) -> Option<&mut PrintJob> {
        self.jobs.iter_mut().find(|j| j.id == id)
    }

    pub fn cancel_job(&mut self, id: u32) -> bool {
        if let Some(job) = self.get_job_mut(id) {
            job.cancel();
            true
        } else {
            false
        }
    }

    pub fn hold_job(&mut self, id: u32) -> bool {
        if let Some(job) = self.get_job_mut(id) {
            job.hold();
            true
        } else {
            false
        }
    }

    pub fn release_job(&mut self, id: u32) -> bool {
        if let Some(job) = self.get_job_mut(id) {
            job.release();
            true
        } else {
            false
        }
    }

    pub fn pause(&mut self) {
        self.paused = true;
    }

    pub fn resume(&mut self) {
        self.paused = false;
    }

    pub fn is_paused(&self) -> bool {
        self.paused
    }

    pub fn job_count(&self) -> usize {
        self.jobs.len()
    }

    pub fn active_jobs(&self) -> Vec<&PrintJob> {
        self.jobs
            .iter()
            .filter(|j| {
                matches!(
                    j.state,
                    JobState::Pending | JobState::PendingHeld | JobState::Processing
                )
            })
            .collect()
    }

    pub fn completed_jobs(&self) -> Vec<&PrintJob> {
        self.jobs
            .iter()
            .filter(|j| {
                matches!(
                    j.state,
                    JobState::Completed | JobState::Canceled | JobState::Aborted
                )
            })
            .collect()
    }

    pub fn purge_completed(&mut self) {
        self.jobs.retain(|j| {
            !matches!(
                j.state,
                JobState::Completed | JobState::Canceled | JobState::Aborted
            )
        });
    }
}

// ===========================================================================
// IPP Protocol — Internet Printing Protocol
// ===========================================================================

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
#[repr(u16)]
pub enum IppOperation {
    PrintJob = 0x0002,
    ValidateJob = 0x0004,
    CreateJob = 0x0005,
    SendDocument = 0x0006,
    SendUri = 0x0007,
    CancelJob = 0x0008,
    GetJobAttributes = 0x0009,
    GetJobs = 0x000A,
    GetPrinterAttributes = 0x000B,
    HoldJob = 0x000C,
    ReleaseJob = 0x000D,
    RestartJob = 0x000E,
    PausePrinter = 0x0010,
    ResumePrinter = 0x0011,
    PurgeJobs = 0x0012,
    SetPrinterAttributes = 0x0013,
    SetJobAttributes = 0x0014,
    GetPrinterSupportedValues = 0x0015,
    CreatePrinterSubscription = 0x0016,
    CreateJobSubscription = 0x0017,
    GetSubscriptionAttributes = 0x0018,
    GetSubscriptions = 0x0019,
    RenewSubscription = 0x001A,
    CancelSubscription = 0x001B,
    GetNotifications = 0x001C,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
#[repr(u16)]
pub enum IppStatus {
    SuccessfulOk = 0x0000,
    SuccessfulOkIgnoredOrSubstituted = 0x0001,
    SuccessfulOkConflicting = 0x0002,
    ClientErrorBadRequest = 0x0400,
    ClientErrorForbidden = 0x0401,
    ClientErrorNotAuthenticated = 0x0402,
    ClientErrorNotAuthorized = 0x0403,
    ClientErrorNotPossible = 0x0404,
    ClientErrorTimeout = 0x0405,
    ClientErrorNotFound = 0x0406,
    ClientErrorGone = 0x0407,
    ClientErrorDocumentFormatNotSupported = 0x040A,
    ClientErrorAttributesOrValuesNotSupported = 0x040B,
    ServerErrorInternalError = 0x0500,
    ServerErrorOperationNotSupported = 0x0501,
    ServerErrorServiceUnavailable = 0x0502,
    ServerErrorVersionNotSupported = 0x0503,
    ServerErrorDeviceError = 0x0504,
    ServerErrorTemporaryError = 0x0505,
    ServerErrorNotAcceptingJobs = 0x0506,
    ServerErrorBusy = 0x0507,
    ServerErrorJobCanceled = 0x0508,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
#[repr(u8)]
pub enum IppValueTag {
    Unsupported = 0x10,
    Unknown = 0x12,
    NoValue = 0x13,
    Integer = 0x21,
    Boolean = 0x22,
    Enum = 0x23,
    OctetString = 0x30,
    DateTime = 0x31,
    Resolution = 0x32,
    RangeOfInteger = 0x33,
    BegCollection = 0x34,
    TextWithLanguage = 0x35,
    NameWithLanguage = 0x36,
    EndCollection = 0x37,
    TextWithoutLanguage = 0x41,
    NameWithoutLanguage = 0x42,
    Keyword = 0x44,
    Uri = 0x45,
    UriScheme = 0x46,
    Charset = 0x47,
    NaturalLanguage = 0x48,
    MimeMediaType = 0x49,
    MemberAttrName = 0x4A,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
#[repr(u8)]
pub enum IppAttributeGroup {
    Operation = 0x01,
    Job = 0x02,
    End = 0x03,
    Printer = 0x04,
    Unsupported = 0x05,
    Subscription = 0x06,
    EventNotification = 0x07,
}

#[derive(Debug, Clone)]
pub enum IppValue {
    Integer(i32),
    Boolean(bool),
    Enum(i32),
    Text(String),
    Name(String),
    Keyword(String),
    Uri(String),
    Charset(String),
    NaturalLanguage(String),
    MimeType(String),
    DateTime([u8; 11]),
    Resolution { x: i32, y: i32, units: u8 },
    RangeOfInteger { lower: i32, upper: i32 },
    OctetString(Vec<u8>),
    Collection(Vec<IppAttribute>),
    NoValue,
}

#[derive(Debug, Clone)]
pub struct IppAttribute {
    pub name: String,
    pub values: Vec<IppValue>,
}

#[derive(Debug, Clone)]
pub struct IppRequest {
    pub version_major: u8,
    pub version_minor: u8,
    pub operation: IppOperation,
    pub request_id: u32,
    pub attributes: Vec<(IppAttributeGroup, Vec<IppAttribute>)>,
    pub document_data: Option<Vec<u8>>,
}

#[derive(Debug, Clone)]
pub struct IppResponse {
    pub version_major: u8,
    pub version_minor: u8,
    pub status: IppStatus,
    pub request_id: u32,
    pub attributes: Vec<(IppAttributeGroup, Vec<IppAttribute>)>,
    pub document_data: Option<Vec<u8>>,
}

impl IppRequest {
    pub fn new(operation: IppOperation, request_id: u32) -> Self {
        Self {
            version_major: 2,
            version_minor: 0,
            operation,
            request_id,
            attributes: Vec::new(),
            document_data: None,
        }
    }

    pub fn add_attribute(&mut self, group: IppAttributeGroup, attr: IppAttribute) {
        if let Some(grp) = self.attributes.iter_mut().find(|g| g.0 == group) {
            grp.1.push(attr);
        } else {
            self.attributes.push((group, alloc::vec![attr]));
        }
    }

    pub fn encode(&self) -> Vec<u8> {
        let mut buf = Vec::new();
        buf.push(self.version_major);
        buf.push(self.version_minor);
        let op = self.operation as u16;
        buf.extend_from_slice(&op.to_be_bytes());
        buf.extend_from_slice(&self.request_id.to_be_bytes());

        for (group, attrs) in &self.attributes {
            buf.push(*group as u8);
            for attr in attrs {
                encode_attribute(&mut buf, attr);
            }
        }
        buf.push(IppAttributeGroup::End as u8);

        if let Some(ref data) = self.document_data {
            buf.extend_from_slice(data);
        }
        buf
    }
}

impl IppResponse {
    pub fn success(request_id: u32) -> Self {
        Self {
            version_major: 2,
            version_minor: 0,
            status: IppStatus::SuccessfulOk,
            request_id,
            attributes: Vec::new(),
            document_data: None,
        }
    }

    pub fn error(request_id: u32, status: IppStatus) -> Self {
        Self {
            version_major: 2,
            version_minor: 0,
            status,
            request_id,
            attributes: Vec::new(),
            document_data: None,
        }
    }

    pub fn add_attribute(&mut self, group: IppAttributeGroup, attr: IppAttribute) {
        if let Some(grp) = self.attributes.iter_mut().find(|g| g.0 == group) {
            grp.1.push(attr);
        } else {
            self.attributes.push((group, alloc::vec![attr]));
        }
    }
}

fn encode_attribute(buf: &mut Vec<u8>, attr: &IppAttribute) {
    for (i, value) in attr.values.iter().enumerate() {
        let tag = value_to_tag(value);
        buf.push(tag);

        if i == 0 {
            let name_bytes = attr.name.as_bytes();
            buf.extend_from_slice(&(name_bytes.len() as u16).to_be_bytes());
            buf.extend_from_slice(name_bytes);
        } else {
            buf.extend_from_slice(&0u16.to_be_bytes());
        }

        match value {
            IppValue::Integer(v) | IppValue::Enum(v) => {
                buf.extend_from_slice(&4u16.to_be_bytes());
                buf.extend_from_slice(&v.to_be_bytes());
            }
            IppValue::Boolean(v) => {
                buf.extend_from_slice(&1u16.to_be_bytes());
                buf.push(if *v { 1 } else { 0 });
            }
            IppValue::Text(s)
            | IppValue::Name(s)
            | IppValue::Keyword(s)
            | IppValue::Uri(s)
            | IppValue::Charset(s)
            | IppValue::NaturalLanguage(s)
            | IppValue::MimeType(s) => {
                let bytes = s.as_bytes();
                buf.extend_from_slice(&(bytes.len() as u16).to_be_bytes());
                buf.extend_from_slice(bytes);
            }
            IppValue::Resolution { x, y, units } => {
                buf.extend_from_slice(&9u16.to_be_bytes());
                buf.extend_from_slice(&x.to_be_bytes());
                buf.extend_from_slice(&y.to_be_bytes());
                buf.push(*units);
            }
            IppValue::RangeOfInteger { lower, upper } => {
                buf.extend_from_slice(&8u16.to_be_bytes());
                buf.extend_from_slice(&lower.to_be_bytes());
                buf.extend_from_slice(&upper.to_be_bytes());
            }
            IppValue::OctetString(data) => {
                buf.extend_from_slice(&(data.len() as u16).to_be_bytes());
                buf.extend_from_slice(data);
            }
            IppValue::DateTime(dt) => {
                buf.extend_from_slice(&11u16.to_be_bytes());
                buf.extend_from_slice(dt);
            }
            IppValue::NoValue => {
                buf.extend_from_slice(&0u16.to_be_bytes());
            }
            IppValue::Collection(_) => {
                buf.extend_from_slice(&0u16.to_be_bytes());
            }
        }
    }
}

fn value_to_tag(value: &IppValue) -> u8 {
    match value {
        IppValue::Integer(_) => IppValueTag::Integer as u8,
        IppValue::Boolean(_) => IppValueTag::Boolean as u8,
        IppValue::Enum(_) => IppValueTag::Enum as u8,
        IppValue::Text(_) => IppValueTag::TextWithoutLanguage as u8,
        IppValue::Name(_) => IppValueTag::NameWithoutLanguage as u8,
        IppValue::Keyword(_) => IppValueTag::Keyword as u8,
        IppValue::Uri(_) => IppValueTag::Uri as u8,
        IppValue::Charset(_) => IppValueTag::Charset as u8,
        IppValue::NaturalLanguage(_) => IppValueTag::NaturalLanguage as u8,
        IppValue::MimeType(_) => IppValueTag::MimeMediaType as u8,
        IppValue::DateTime(_) => IppValueTag::DateTime as u8,
        IppValue::Resolution { .. } => IppValueTag::Resolution as u8,
        IppValue::RangeOfInteger { .. } => IppValueTag::RangeOfInteger as u8,
        IppValue::OctetString(_) => IppValueTag::OctetString as u8,
        IppValue::Collection(_) => IppValueTag::BegCollection as u8,
        IppValue::NoValue => IppValueTag::NoValue as u8,
    }
}

// ===========================================================================
// Page Description — layout, margins, imposition
// ===========================================================================

#[derive(Debug, Clone, Copy)]
pub struct Margins {
    pub top: u32,
    pub bottom: u32,
    pub left: u32,
    pub right: u32,
}

impl Margins {
    pub const fn default_margins() -> Self {
        Self {
            top: 72,
            bottom: 72,
            left: 72,
            right: 72,
        }
    }

    pub const fn zero() -> Self {
        Self {
            top: 0,
            bottom: 0,
            left: 0,
            right: 0,
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ScalingMode {
    None,
    FitToPage,
    ShrinkToFit,
    FillPage,
    CustomPercent(u32),
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum NUpLayout {
    LeftToRightTopToBottom,
    RightToLeftTopToBottom,
    TopToBottomLeftToRight,
    TopToBottomRightToLeft,
}

#[derive(Debug, Clone, Copy)]
pub struct PageLayout {
    pub media: MediaSize,
    pub orientation: Orientation,
    pub margins: Margins,
    pub scaling: ScalingMode,
    pub n_up: u32,
    pub n_up_layout: NUpLayout,
    pub booklet: bool,
    pub mirror: bool,
}

impl PageLayout {
    pub fn new(media: MediaSize) -> Self {
        Self {
            media,
            orientation: Orientation::Portrait,
            margins: Margins::default_margins(),
            scaling: ScalingMode::None,
            n_up: 1,
            n_up_layout: NUpLayout::LeftToRightTopToBottom,
            booklet: false,
            mirror: false,
        }
    }

    pub fn printable_width(&self) -> u32 {
        self.media
            .width_points()
            .saturating_sub(self.margins.left + self.margins.right)
    }

    pub fn printable_height(&self) -> u32 {
        self.media
            .height_points()
            .saturating_sub(self.margins.top + self.margins.bottom)
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum RasterFormat {
    Rgb8,
    Rgba8,
    Cmyk8,
    Gray8,
    Gray16,
    Rgb16,
}

#[derive(Debug, Clone)]
pub struct RasterPage {
    pub width: u32,
    pub height: u32,
    pub format: RasterFormat,
    pub dpi: Resolution,
    pub data: Vec<u8>,
}

// ===========================================================================
// PPD — PostScript Printer Description parsing
// ===========================================================================

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum PpdValueType {
    String(String),
    InvocationValue(String),
    QuotedValue(String),
    Symbol(String),
}

#[derive(Debug, Clone)]
pub struct PpdOption {
    pub keyword: String,
    pub default_choice: String,
    pub choices: Vec<PpdChoice>,
    pub ui_type: PpdUiType,
    pub group: String,
    pub order_dependency: u32,
}

#[derive(Debug, Clone)]
pub struct PpdChoice {
    pub name: String,
    pub text: String,
    pub code: String,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum PpdUiType {
    Boolean,
    PickOne,
    PickMany,
}

#[derive(Debug, Clone)]
pub struct PpdConstraint {
    pub option1: String,
    pub choice1: String,
    pub option2: String,
    pub choice2: String,
}

#[derive(Debug, Clone)]
pub struct PpdUiGroup {
    pub name: String,
    pub text: String,
    pub options: Vec<String>,
}

#[derive(Debug, Clone)]
pub struct PpdFile {
    pub nickname: String,
    pub manufacturer: String,
    pub model_name: String,
    pub pcfilename: String,
    pub language_level: u32,
    pub color_device: bool,
    pub default_resolution: Resolution,
    pub options: Vec<PpdOption>,
    pub constraints: Vec<PpdConstraint>,
    pub ui_groups: Vec<PpdUiGroup>,
    pub fonts: Vec<String>,
    pub file_version: String,
}

impl PpdFile {
    pub fn new() -> Self {
        Self {
            nickname: String::new(),
            manufacturer: String::new(),
            model_name: String::new(),
            pcfilename: String::new(),
            language_level: 2,
            color_device: false,
            default_resolution: Resolution {
                x_dpi: 300,
                y_dpi: 300,
            },
            options: Vec::new(),
            constraints: Vec::new(),
            ui_groups: Vec::new(),
            fonts: Vec::new(),
            file_version: String::from("4.3"),
        }
    }

    pub fn get_option(&self, keyword: &str) -> Option<&PpdOption> {
        self.options.iter().find(|o| o.keyword == keyword)
    }

    pub fn evaluate_constraint(
        &self,
        constraint: &PpdConstraint,
        selections: &BTreeMap<String, String>,
    ) -> bool {
        let sel1 = selections.get(&constraint.option1);
        let sel2 = selections.get(&constraint.option2);

        match (sel1, sel2) {
            (Some(c1), Some(c2)) => *c1 == constraint.choice1 && *c2 == constraint.choice2,
            _ => false,
        }
    }

    pub fn check_all_constraints(
        &self,
        selections: &BTreeMap<String, String>,
    ) -> Vec<&PpdConstraint> {
        self.constraints
            .iter()
            .filter(|c| self.evaluate_constraint(c, selections))
            .collect()
    }

    pub fn parse_line(&mut self, line: &str) {
        let trimmed = line.trim();
        if trimmed.starts_with("*NickName:") {
            self.nickname = extract_quoted(trimmed);
        } else if trimmed.starts_with("*Manufacturer:") {
            self.manufacturer = extract_quoted(trimmed);
        } else if trimmed.starts_with("*ModelName:") {
            self.model_name = extract_quoted(trimmed);
        } else if trimmed.starts_with("*PCFileName:") {
            self.pcfilename = extract_quoted(trimmed);
        } else if trimmed.starts_with("*ColorDevice:") {
            self.color_device = trimmed.contains("True");
        } else if trimmed.starts_with("*LanguageLevel:") {
            if let Some(val) = extract_quoted_or_value(trimmed).parse::<u32>().ok() {
                self.language_level = val;
            }
        }
    }
}

fn extract_quoted(line: &str) -> String {
    if let Some(start) = line.find('"') {
        if let Some(end) = line[start + 1..].find('"') {
            return String::from(&line[start + 1..start + 1 + end]);
        }
    }
    String::new()
}

fn extract_quoted_or_value(line: &str) -> String {
    let quoted = extract_quoted(line);
    if !quoted.is_empty() {
        return quoted;
    }
    if let Some(colon) = line.find(':') {
        return String::from(line[colon + 1..].trim().trim_matches('"'));
    }
    String::new()
}

// ===========================================================================
// Print Filters — document conversion pipeline
// ===========================================================================

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum FilterType {
    TextToPostScript,
    ImageScaling,
    PdfToRaster,
    PostScriptToRaster,
    PageImposition,
    ColorConversion,
    RasterToPcl,
    RasterToEscP,
}

#[derive(Debug, Clone)]
pub struct FilterChainEntry {
    pub filter_type: FilterType,
    pub input_mime: String,
    pub output_mime: String,
    pub cost: u32,
}

pub struct FilterChain {
    filters: Vec<FilterChainEntry>,
}

impl FilterChain {
    pub fn new() -> Self {
        Self {
            filters: Vec::new(),
        }
    }

    pub fn add_filter(&mut self, entry: FilterChainEntry) {
        self.filters.push(entry);
    }

    pub fn find_chain(&self, input: &str, output: &str) -> Vec<&FilterChainEntry> {
        let mut chain = Vec::new();
        let mut current_mime = input;

        for filter in &self.filters {
            if filter.input_mime == current_mime {
                chain.push(filter);
                current_mime = &filter.output_mime;
                if current_mime == output {
                    return chain;
                }
            }
        }
        chain
    }

    pub fn text_to_postscript(text: &[u8], layout: &PageLayout) -> Vec<u8> {
        let mut ps = Vec::new();
        ps.extend_from_slice(b"%!PS-Adobe-3.0\n");
        ps.extend_from_slice(b"%%Creator: RaePrint\n");
        let media_w = layout.media.width_points();
        let media_h = layout.media.height_points();
        let bbox = alloc::format!("%%BoundingBox: 0 0 {} {}\n", media_w, media_h);
        ps.extend_from_slice(bbox.as_bytes());
        ps.extend_from_slice(b"%%Pages: 1\n");
        ps.extend_from_slice(b"%%EndComments\n\n");
        ps.extend_from_slice(b"/Courier findfont 12 scalefont setfont\n");

        let x = layout.margins.left;
        let mut y = media_h.saturating_sub(layout.margins.top);
        let line_height: u32 = 14;

        ps.extend_from_slice(b"%%Page: 1 1\n");

        for line in text.split(|&b| b == b'\n') {
            if y < layout.margins.bottom {
                ps.extend_from_slice(b"showpage\n");
                y = media_h.saturating_sub(layout.margins.top);
                ps.extend_from_slice(b"%%Page: 2 2\n");
            }
            let moveto = alloc::format!("{} {} moveto\n", x, y);
            ps.extend_from_slice(moveto.as_bytes());
            ps.push(b'(');
            for &ch in line {
                match ch {
                    b'(' => ps.extend_from_slice(b"\\("),
                    b')' => ps.extend_from_slice(b"\\)"),
                    b'\\' => ps.extend_from_slice(b"\\\\"),
                    c if c >= 0x20 && c < 0x7F => ps.push(c),
                    _ => ps.push(b'.'),
                }
            }
            ps.extend_from_slice(b") show\n");
            y = y.saturating_sub(line_height);
        }

        ps.extend_from_slice(b"showpage\n");
        ps.extend_from_slice(b"%%EOF\n");
        ps
    }

    pub fn scale_image(
        data: &[u8],
        src_w: u32,
        src_h: u32,
        dst_w: u32,
        dst_h: u32,
        bpp: u32,
    ) -> Vec<u8> {
        let dst_size = (dst_w * dst_h * bpp) as usize;
        let mut output = Vec::with_capacity(dst_size);

        let x_ratio = (src_w as f64) / (dst_w as f64);
        let y_ratio = (src_h as f64) / (dst_h as f64);

        for y in 0..dst_h {
            for x in 0..dst_w {
                let src_x = (x as f64 * x_ratio) as u32;
                let src_y = (y as f64 * y_ratio) as u32;
                let src_idx = ((src_y * src_w + src_x) * bpp) as usize;

                for c in 0..bpp as usize {
                    if src_idx + c < data.len() {
                        output.push(data[src_idx + c]);
                    } else {
                        output.push(0);
                    }
                }
            }
        }
        output
    }
}

// ===========================================================================
// Spooler — spool directory and temporary file management
// ===========================================================================

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum SpoolFileState {
    Writing,
    Ready,
    Printing,
    Done,
    Error,
}

#[derive(Debug, Clone)]
pub struct SpoolEntry {
    pub id: u32,
    pub job_id: u32,
    pub filename: String,
    pub size: u64,
    pub state: SpoolFileState,
    pub created_at: u64,
    pub retry_count: u32,
    pub max_retries: u32,
}

pub struct Spooler {
    spool_dir: String,
    entries: Vec<SpoolEntry>,
    next_spool_id: AtomicU32,
    max_spool_size: u64,
    current_spool_size: u64,
}

impl Spooler {
    pub fn new(spool_dir: String) -> Self {
        Self {
            spool_dir,
            entries: Vec::new(),
            next_spool_id: AtomicU32::new(1),
            max_spool_size: 1024 * 1024 * 1024,
            current_spool_size: 0,
        }
    }

    pub fn create_spool_file(&mut self, job_id: u32, size: u64, timestamp: u64) -> Option<u32> {
        if self.current_spool_size + size > self.max_spool_size {
            return None;
        }
        let id = self.next_spool_id.fetch_add(1, Ordering::Relaxed);
        let filename = alloc::format!("{}/spool-{:08x}.prn", self.spool_dir, id);
        self.entries.push(SpoolEntry {
            id,
            job_id,
            filename,
            size,
            state: SpoolFileState::Writing,
            created_at: timestamp,
            retry_count: 0,
            max_retries: 3,
        });
        self.current_spool_size += size;
        Some(id)
    }

    pub fn mark_ready(&mut self, id: u32) -> bool {
        if let Some(entry) = self.entries.iter_mut().find(|e| e.id == id) {
            entry.state = SpoolFileState::Ready;
            true
        } else {
            false
        }
    }

    pub fn mark_printing(&mut self, id: u32) -> bool {
        if let Some(entry) = self.entries.iter_mut().find(|e| e.id == id) {
            entry.state = SpoolFileState::Printing;
            true
        } else {
            false
        }
    }

    pub fn mark_done(&mut self, id: u32) -> bool {
        if let Some(entry) = self.entries.iter_mut().find(|e| e.id == id) {
            entry.state = SpoolFileState::Done;
            true
        } else {
            false
        }
    }

    pub fn mark_error(&mut self, id: u32) -> bool {
        if let Some(entry) = self.entries.iter_mut().find(|e| e.id == id) {
            entry.state = SpoolFileState::Error;
            entry.retry_count += 1;
            true
        } else {
            false
        }
    }

    pub fn should_retry(&self, id: u32) -> bool {
        self.entries
            .iter()
            .find(|e| e.id == id)
            .map(|e| e.retry_count < e.max_retries)
            .unwrap_or(false)
    }

    pub fn retry(&mut self, id: u32) -> bool {
        if let Some(entry) = self.entries.iter_mut().find(|e| e.id == id) {
            if entry.retry_count < entry.max_retries {
                entry.state = SpoolFileState::Ready;
                return true;
            }
        }
        false
    }

    pub fn cleanup_done(&mut self) {
        let removed_size: u64 = self
            .entries
            .iter()
            .filter(|e| e.state == SpoolFileState::Done)
            .map(|e| e.size)
            .sum();
        self.entries.retain(|e| e.state != SpoolFileState::Done);
        self.current_spool_size = self.current_spool_size.saturating_sub(removed_size);
    }

    pub fn pending_count(&self) -> usize {
        self.entries
            .iter()
            .filter(|e| e.state == SpoolFileState::Ready)
            .count()
    }

    pub fn total_spool_usage(&self) -> u64 {
        self.current_spool_size
    }
}

// ===========================================================================
// Print Dialog Data Model
// ===========================================================================

#[derive(Debug, Clone)]
pub struct PrintDialogSettings {
    pub printer_name: String,
    pub copies: u32,
    pub collate: bool,
    pub page_range: PrintPageSelection,
    pub orientation: Orientation,
    pub media: MediaSize,
    pub color_mode: ColorMode,
    pub duplex: DuplexMode,
    pub quality: PrintQuality,
    pub n_up: u32,
    pub reverse_order: bool,
    pub fit_to_page: bool,
    pub paper_tray: Option<String>,
}

#[derive(Debug, Clone)]
pub enum PrintPageSelection {
    All,
    CurrentPage,
    Range(Vec<PageRange>),
    Selection,
}

impl PrintDialogSettings {
    pub fn defaults(printer_name: String) -> Self {
        Self {
            printer_name,
            copies: 1,
            collate: true,
            page_range: PrintPageSelection::All,
            orientation: Orientation::Portrait,
            media: MediaSize::Letter,
            color_mode: ColorMode::AutoDetect,
            duplex: DuplexMode::None,
            quality: PrintQuality::Normal,
            n_up: 1,
            reverse_order: false,
            fit_to_page: false,
            paper_tray: None,
        }
    }

    pub fn to_job(&self, document: PrintDocument, owner: String) -> PrintJob {
        let mut job = PrintJob::new(0, self.printer_name.clone(), document, owner);
        job.copies = self.copies;
        job.collate = self.collate;
        job.orientation = self.orientation;
        job.media = self.media;
        job.color_mode = self.color_mode;
        job.duplex = self.duplex;
        job.quality = self.quality;
        job.n_up = self.n_up;
        job.reverse_order = self.reverse_order;
        job.fit_to_page = self.fit_to_page;
        if let PrintPageSelection::Range(ref ranges) = self.page_range {
            job.page_ranges = Some(ranges.clone());
        }
        job
    }
}

// ===========================================================================
// Global Print Manager
// ===========================================================================

pub struct PrintManager {
    pub discovery: PrinterDiscovery,
    pub queues: BTreeMap<String, PrintQueue>,
    pub spooler: Spooler,
    pub filter_chain: FilterChain,
    pub default_printer: Option<String>,
    pub total_jobs_submitted: AtomicU64,
    pub total_pages_printed: AtomicU64,
    pub initialized: bool,
}

impl PrintManager {
    pub fn new() -> Self {
        let mut filter_chain = FilterChain::new();
        filter_chain.add_filter(FilterChainEntry {
            filter_type: FilterType::TextToPostScript,
            input_mime: String::from("text/plain"),
            output_mime: String::from("application/postscript"),
            cost: 100,
        });
        filter_chain.add_filter(FilterChainEntry {
            filter_type: FilterType::PostScriptToRaster,
            input_mime: String::from("application/postscript"),
            output_mime: String::from("image/pwg-raster"),
            cost: 200,
        });
        filter_chain.add_filter(FilterChainEntry {
            filter_type: FilterType::PdfToRaster,
            input_mime: String::from("application/pdf"),
            output_mime: String::from("image/pwg-raster"),
            cost: 150,
        });
        filter_chain.add_filter(FilterChainEntry {
            filter_type: FilterType::ImageScaling,
            input_mime: String::from("image/png"),
            output_mime: String::from("image/pwg-raster"),
            cost: 50,
        });
        filter_chain.add_filter(FilterChainEntry {
            filter_type: FilterType::RasterToPcl,
            input_mime: String::from("image/pwg-raster"),
            output_mime: String::from("application/vnd.hp-pcl"),
            cost: 75,
        });

        Self {
            discovery: PrinterDiscovery::new(),
            queues: BTreeMap::new(),
            spooler: Spooler::new(String::from("/var/spool/raeprint")),
            filter_chain,
            default_printer: None,
            total_jobs_submitted: AtomicU64::new(0),
            total_pages_printed: AtomicU64::new(0),
            initialized: false,
        }
    }

    pub fn add_printer(&mut self, info: PrinterInfo) {
        let name = info.name.clone();
        self.discovery.add_discovered(info);
        self.queues
            .insert(name.clone(), PrintQueue::new(name.clone()));
        if self.default_printer.is_none() {
            self.default_printer = Some(name);
        }
    }

    pub fn remove_printer(&mut self, name: &str) {
        self.queues.remove(name);
        if self.default_printer.as_deref() == Some(name) {
            self.default_printer = self.queues.keys().next().cloned();
        }
    }

    pub fn submit_job(&mut self, printer: &str, job: PrintJob) -> Option<u32> {
        self.total_jobs_submitted.fetch_add(1, Ordering::Relaxed);
        if let Some(queue) = self.queues.get_mut(printer) {
            Some(queue.submit_job(job))
        } else {
            None
        }
    }

    pub fn set_default_printer(&mut self, name: String) {
        self.default_printer = Some(name);
    }

    pub fn get_queue(&self, printer: &str) -> Option<&PrintQueue> {
        self.queues.get(printer)
    }

    pub fn get_queue_mut(&mut self, printer: &str) -> Option<&mut PrintQueue> {
        self.queues.get_mut(printer)
    }

    pub fn printer_count(&self) -> usize {
        self.queues.len()
    }

    pub fn total_pending_jobs(&self) -> usize {
        self.queues.values().map(|q| q.active_jobs().len()).sum()
    }
}

pub static PRINT_MANAGER: spin::Mutex<Option<PrintManager>> = spin::Mutex::new(None);

pub fn init() {
    let mut mgr = PrintManager::new();
    mgr.initialized = true;
    *PRINT_MANAGER.lock() = Some(mgr);
}

// ===========================================================================
// Host KAT suite — FAIL-able, concrete-value asserts (cargo test -p raeprint)
// ===========================================================================
#[cfg(test)]
mod tests {
    use super::pdf::{fit_to_page, n_up_cell, n_up_grid, BaseFont, PdfBuilder, PdfError, PdfPage};
    use super::*;
    use alloc::string::{String, ToString};
    use alloc::vec;

    // Tiny helper: does `haystack` contain `needle` as a byte subsequence?
    fn contains(haystack: &[u8], needle: &[u8]) -> bool {
        if needle.is_empty() || needle.len() > haystack.len() {
            return needle.is_empty();
        }
        haystack.windows(needle.len()).any(|w| w == needle)
    }

    // Find the integer that follows the last "startxref\n" in the PDF.
    fn parse_startxref(pdf: &[u8]) -> Option<usize> {
        let key = b"startxref\n";
        let pos = pdf
            .windows(key.len())
            .enumerate()
            .filter(|(_, w)| *w == key)
            .map(|(i, _)| i)
            .last()?;
        let mut i = pos + key.len();
        let mut val: usize = 0;
        let mut any = false;
        while i < pdf.len() && pdf[i].is_ascii_digit() {
            val = val * 10 + (pdf[i] - b'0') as usize;
            i += 1;
            any = true;
        }
        if any {
            Some(val)
        } else {
            None
        }
    }

    #[test]
    fn pdf_has_valid_magic_and_eof() {
        let pdf = PdfBuilder::single_text_page("Hello AthenaOS", BaseFont::Helvetica)
            .build()
            .expect("single page must build");
        assert!(pdf.starts_with(b"%PDF-1.7"), "missing %PDF magic");
        assert!(contains(&pdf, b"%%EOF"), "missing %%EOF trailer marker");
        assert!(contains(&pdf, b"trailer"), "missing trailer keyword");
    }

    #[test]
    fn pdf_startxref_points_at_xref_keyword() {
        let pdf = PdfBuilder::single_text_page("line one\nline two", BaseFont::Courier)
            .build()
            .unwrap();
        let off = parse_startxref(&pdf).expect("startxref offset must parse");
        // The bytes at the startxref offset must be the literal "xref\n".
        assert!(off < pdf.len(), "startxref offset {} out of range", off);
        assert_eq!(
            &pdf[off..off + 5],
            b"xref\n",
            "startxref must point at the xref keyword"
        );
    }

    #[test]
    fn pdf_object_and_page_counts_match() {
        // 3 pages -> Count 3, objects = 3 + 3*2 = 9, Size = 10.
        let layout = PageLayout::new(MediaSize::A4);
        let mut b = PdfBuilder::new();
        for n in 0..3 {
            b.add_page(PdfPage::from_text(
                layout,
                &alloc::format!("page {}", n),
                BaseFont::Helvetica,
            ));
        }
        let pdf = b.build().unwrap();
        assert!(contains(&pdf, b"/Count 3"), "Pages /Count must be 3");
        assert!(
            contains(&pdf, b"/Size 10"),
            "trailer /Size must be 10 for 9 objects"
        );
        // xref subsection header declares 10 entries (0..=9).
        assert!(
            contains(&pdf, b"xref\n0 10\n"),
            "xref must declare 10 entries"
        );
        // The free-list head entry must be exactly the spec form.
        assert!(
            contains(&pdf, b"0000000000 65535 f \n"),
            "missing canonical free-object xref entry"
        );
    }

    #[test]
    fn pdf_content_stream_carries_requested_text() {
        let pdf = PdfBuilder::single_text_page("UNIQUEMARKER42", BaseFont::Helvetica)
            .build()
            .unwrap();
        assert!(
            contains(&pdf, b"(UNIQUEMARKER42) Tj"),
            "requested text must appear in a content stream show op"
        );
        assert!(contains(&pdf, b"/Type /Catalog"), "missing Catalog object");
        assert!(contains(&pdf, b"/BaseFont /Helvetica"), "missing font");
    }

    #[test]
    fn pdf_escapes_special_chars() {
        let pdf = PdfBuilder::single_text_page("a(b)c\\d", BaseFont::Courier)
            .build()
            .unwrap();
        // '(' ')' '\' must be backslash-escaped inside the literal string.
        assert!(
            contains(&pdf, b"(a\\(b\\)c\\\\d) Tj"),
            "special chars must be escaped in PDF string literals"
        );
    }

    #[test]
    fn pdf_compressed_uses_flatedecode_and_roundtrips() {
        let text = "compress me ".to_string().repeat(50);
        let pdf = {
            let mut b = PdfBuilder::compressed();
            b.add_page(PdfPage::from_text(
                PageLayout::new(MediaSize::Letter),
                &text,
                BaseFont::Helvetica,
            ));
            b.build().unwrap()
        };
        assert!(
            contains(&pdf, b"/Filter /FlateDecode"),
            "compressed PDF must declare the FlateDecode filter"
        );
        // Locate one stream..endstream and verify it zlib-decompresses.
        let start = pdf
            .windows(7)
            .position(|w| w == b"stream\n")
            .expect("a stream must exist")
            + 7;
        let end = start
            + pdf[start..]
                .windows(10)
                .position(|w| w == b"\nendstream")
                .expect("endstream must follow");
        let decoded = rae_deflate::zlib_decompress(&pdf[start..end])
            .expect("FlateDecode stream must inflate");
        assert!(
            contains(&decoded, b"(compress me "),
            "inflated content stream must contain the source text"
        );
    }

    #[test]
    fn pdf_landscape_swaps_mediabox() {
        let mut layout = PageLayout::new(MediaSize::Letter); // 612 x 792 portrait
        layout.orientation = Orientation::Landscape;
        let pdf = {
            let mut b = PdfBuilder::new();
            b.add_page(PdfPage::from_text(layout, "x", BaseFont::Helvetica));
            b.build().unwrap()
        };
        assert!(
            contains(&pdf, b"/MediaBox [ 0 0 792 612 ]"),
            "landscape Letter MediaBox must be 792 x 612"
        );
    }

    #[test]
    fn pdf_empty_document_is_clean_err() {
        let b = PdfBuilder::new();
        assert_eq!(b.build(), Err(PdfError::NoPages));
    }

    #[test]
    fn pdf_degenerate_media_is_clean_err() {
        let layout = PageLayout::new(MediaSize::Custom {
            width_mm: 0,
            height_mm: 0,
        });
        let mut b = PdfBuilder::new();
        b.add_page(PdfPage::from_text(layout, "x", BaseFont::Helvetica));
        assert_eq!(b.build(), Err(PdfError::DegenerateMedia));
    }

    // ---- Page-setup geometry ----

    #[test]
    fn fit_to_page_shrinks_and_centers() {
        // 1000x1000 content into a 500x800 area: limited by width -> scale 0.5.
        let f = fit_to_page(1000, 1000, 500, 800).unwrap();
        assert_eq!(f.scale_permille, 500);
        assert_eq!(f.scaled_w, 500);
        assert_eq!(f.scaled_h, 500);
        assert_eq!(f.offset_x, 0); // fills width exactly
        assert_eq!(f.offset_y, 150); // (800-500)/2
    }

    #[test]
    fn fit_to_page_never_upscales() {
        // Tiny content into a big area stays 100%.
        let f = fit_to_page(100, 100, 1000, 1000).unwrap();
        assert_eq!(f.scale_permille, 1000);
        assert_eq!(f.scaled_w, 100);
        assert_eq!(f.offset_x, 450); // (1000-100)/2
    }

    #[test]
    fn fit_to_page_degenerate_is_none() {
        assert_eq!(fit_to_page(0, 100, 100, 100), None);
        assert_eq!(fit_to_page(100, 100, 0, 100), None);
    }

    #[test]
    fn n_up_grid_presets_and_cells() {
        assert_eq!(n_up_grid(1), (1, 1));
        assert_eq!(n_up_grid(2), (1, 2));
        assert_eq!(n_up_grid(4), (2, 2));
        assert_eq!(n_up_grid(9), (3, 3));
        // Non-preset: 7 -> ceil(sqrt)=3 cols, ceil(7/3)=3 rows.
        assert_eq!(n_up_grid(7), (3, 3));
        // A4 printable 612x720 split 2x2 -> 306x360 cells.
        assert_eq!(n_up_cell(612, 720, 2, 2), Some((306, 360)));
        assert_eq!(n_up_cell(612, 720, 0, 2), None);
    }

    #[test]
    fn printable_area_subtracts_margins() {
        let layout = PageLayout::new(MediaSize::Letter); // 612x792, margins 72 each
        assert_eq!(layout.printable_width(), 612 - 144);
        assert_eq!(layout.printable_height(), 792 - 144);
    }

    // ---- Spool state machine ----

    #[test]
    fn spool_state_machine_transitions() {
        let mut sp = Spooler::new(String::from("/tmp/spool"));
        let id = sp.create_spool_file(7, 1024, 100).expect("spool file");
        assert_eq!(sp.total_spool_usage(), 1024);
        assert_eq!(sp.pending_count(), 0); // Writing, not Ready yet.
        assert!(sp.mark_ready(id));
        assert_eq!(sp.pending_count(), 1);
        assert!(sp.mark_printing(id));
        assert_eq!(sp.pending_count(), 0);
        assert!(sp.mark_done(id));
        sp.cleanup_done();
        assert_eq!(sp.total_spool_usage(), 0);
        // Unknown id -> false, never panics.
        assert!(!sp.mark_ready(9999));
    }

    #[test]
    fn spool_retry_respects_max() {
        let mut sp = Spooler::new(String::from("/tmp/spool"));
        let id = sp.create_spool_file(1, 10, 0).unwrap();
        // Fail it up to max_retries (3) times.
        for _ in 0..3 {
            assert!(sp.mark_error(id));
        }
        assert!(!sp.should_retry(id), "exhausted retries must not retry");
        assert!(!sp.retry(id), "retry past max must fail cleanly");
    }

    #[test]
    fn spool_over_capacity_returns_none() {
        let mut sp = Spooler::new(String::from("/tmp/spool"));
        // max_spool_size is 1 GiB; request 2 GiB -> None, not a panic.
        let huge = 2u64 * 1024 * 1024 * 1024;
        assert_eq!(sp.create_spool_file(1, huge, 0), None);
    }

    // ---- Print-job lifecycle / queue ----

    #[test]
    fn job_state_machine_and_sheets() {
        let doc = PrintDocument {
            name: String::from("d"),
            mime_type: String::from("application/pdf"),
            data: vec![],
            total_pages: Some(10),
        };
        let mut job = PrintJob::new(1, String::from("p"), doc, String::from("u"));
        job.copies = 2;
        job.duplex = DuplexMode::LongEdge;
        // 10 pages duplex = 5 sheets/copy * 2 copies = 10 sheets.
        assert_eq!(job.total_sheets_needed(), 10);

        job.hold();
        assert_eq!(job.state, JobState::PendingHeld);
        job.release();
        assert_eq!(job.state, JobState::Pending);
        job.start_processing(5);
        assert_eq!(job.state, JobState::Processing);
        assert_eq!(job.started_at, Some(5));
        job.cancel();
        assert_eq!(job.state, JobState::Canceled);
        // Completed/canceled jobs cannot be re-canceled into another state.
        job.cancel();
        assert_eq!(job.state, JobState::Canceled);
    }

    #[test]
    fn queue_priority_ordering() {
        let mut q = PrintQueue::new(String::from("p"));
        let mk = |prio: u32| {
            let doc = PrintDocument {
                name: String::new(),
                mime_type: String::new(),
                data: vec![],
                total_pages: Some(1),
            };
            let mut j = PrintJob::new(0, String::from("p"), doc, String::from("u"));
            j.priority = prio;
            j
        };
        q.submit_job(mk(10));
        q.submit_job(mk(90));
        q.submit_job(mk(50));
        // Highest priority (90) should be picked first.
        let next = q.next_job().expect("a pending job");
        assert_eq!(next.priority, 90);
        assert_eq!(q.job_count(), 3);
    }

    #[test]
    fn queue_capacity_back_pressure() {
        let mut q = PrintQueue::new(String::from("p"));
        assert_eq!(q.max_jobs(), 1000);
        let mk = || {
            let doc = PrintDocument {
                name: String::new(),
                mime_type: String::new(),
                data: vec![],
                total_pages: Some(1),
            };
            PrintJob::new(0, String::from("p"), doc, String::from("u"))
        };
        // try_submit_job accepts up to capacity then refuses cleanly.
        for _ in 0..1000 {
            assert!(q.try_submit_job(mk()).is_some());
        }
        assert!(
            q.try_submit_job(mk()).is_none(),
            "over-capacity must be None"
        );
        assert_eq!(q.job_count(), 1000);
    }

    #[test]
    fn queue_pause_blocks_next_job() {
        let mut q = PrintQueue::new(String::from("p"));
        let doc = PrintDocument {
            name: String::new(),
            mime_type: String::new(),
            data: vec![],
            total_pages: Some(1),
        };
        q.submit_job(PrintJob::new(0, String::from("p"), doc, String::from("u")));
        q.pause();
        assert!(q.next_job().is_none(), "paused queue yields no job");
        q.resume();
        assert!(q.next_job().is_some(), "resumed queue yields the job");
    }
}
