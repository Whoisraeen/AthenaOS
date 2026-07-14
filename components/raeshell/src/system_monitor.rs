//! System monitor for RaeenOS — task manager with CPU/GPU/memory/disk/network
//! graphs, process list, and per-core utilisation.

#![allow(dead_code)]

extern crate alloc;

use alloc::format;
use alloc::string::String;
use alloc::vec::Vec;

// ── Colour palette ───────────────────────────────────────────────────────

const MON_BG: u32 = 0xFF_10_12_1E;
const MON_PANEL_BG: u32 = 0xFF_14_16_22;
const MON_HEADER_BG: u32 = 0xFF_18_1A_28;
const MON_FG: u32 = 0xFF_D0_D0_E0;
const MON_DIM: u32 = 0xFF_70_70_80;
const MON_ACCENT: u32 = 0xFF_4E_9C_FF;
const MON_GREEN: u32 = 0xFF_44_DD_66;
const MON_YELLOW: u32 = 0xFF_FF_CC_33;
const MON_RED: u32 = 0xFF_FF_44_44;
const MON_ORANGE: u32 = 0xFF_FF_88_33;
const MON_PURPLE: u32 = 0xFF_BB_66_FF;
const MON_CYAN: u32 = 0xFF_44_CC_CC;
const MON_BORDER: u32 = 0xFF_33_33_55;
const MON_SELECTED: u32 = 0xFF_28_2C_44;
const MON_GRAPH_BG: u32 = 0xFF_0C_0E_18;
const GLYPH_W: usize = 8;
const GLYPH_H: usize = 8;

// ── Data structures ──────────────────────────────────────────────────────

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum MonitorTab {
    Overview,
    Processes,
    Performance,
    Disks,
    Network,
    Gpu,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ProcessColumn {
    Pid,
    Name,
    Cpu,
    Memory,
    Threads,
    DiskRead,
    DiskWrite,
    User,
    Priority,
}

#[derive(Debug, Clone)]
pub struct ProcessInfo {
    pub pid: u64,
    pub name: String,
    pub state: String,
    pub cpu_percent: f32,
    pub memory_bytes: u64,
    pub memory_percent: f32,
    pub threads: u32,
    pub disk_read_bytes: u64,
    pub disk_write_bytes: u64,
    pub net_rx_bytes: u64,
    pub net_tx_bytes: u64,
    pub user: String,
    pub priority: String,
    pub start_time: u64,
    pub command_line: String,
}

#[derive(Debug, Clone)]
pub struct CpuSnapshot {
    pub timestamp: u64,
    pub per_core: Vec<f32>,
    pub total: f32,
    pub temp_c: Option<f32>,
    pub frequency_mhz: u32,
}

#[derive(Debug, Clone, Copy)]
pub struct MemoryInfo {
    pub total: u64,
    pub used: u64,
    pub cached: u64,
    pub buffers: u64,
    pub swap_total: u64,
    pub swap_used: u64,
}

#[derive(Debug, Clone)]
pub struct GpuMonitorInfo {
    pub name: String,
    pub usage_percent: f32,
    pub vram_used: u64,
    pub vram_total: u64,
    pub temp_c: f32,
    pub power_w: f32,
    pub clock_mhz: u32,
    pub fan_percent: u8,
}

#[derive(Debug, Clone)]
pub struct DiskInfo {
    pub name: String,
    pub mount: String,
    pub total: u64,
    pub used: u64,
    pub read_bps: u64,
    pub write_bps: u64,
    pub iops: u32,
    pub temp_c: Option<u8>,
}

#[derive(Debug, Clone)]
pub struct NetworkInterfaceInfo {
    pub name: String,
    pub ip: String,
    pub rx_bps: u64,
    pub tx_bps: u64,
    pub rx_total: u64,
    pub tx_total: u64,
    pub connected: bool,
}

// ── Ring buffer for time-series data ─────────────────────────────────────

pub struct RingBuffer<T> {
    pub data: Vec<T>,
    head: usize,
    capacity: usize,
}

impl<T: Clone> RingBuffer<T> {
    pub fn new(capacity: usize) -> Self {
        Self {
            data: Vec::new(),
            head: 0,
            capacity,
        }
    }

    pub fn push(&mut self, item: T) {
        if self.data.len() < self.capacity {
            self.data.push(item);
            self.head = self.data.len() - 1;
        } else {
            self.head = (self.head + 1) % self.capacity;
            self.data[self.head] = item;
        }
    }

    pub fn len(&self) -> usize {
        self.data.len()
    }

    pub fn is_empty(&self) -> bool {
        self.data.is_empty()
    }

    pub fn latest(&self) -> Option<&T> {
        if self.data.is_empty() {
            None
        } else {
            Some(&self.data[self.head])
        }
    }

    pub fn iter_chronological(&self) -> impl Iterator<Item = &T> {
        let len = self.data.len();
        let start = if len < self.capacity {
            0
        } else {
            (self.head + 1) % self.capacity
        };
        (0..len).map(move |i| &self.data[(start + i) % len])
    }
}

// ── System monitor ───────────────────────────────────────────────────────

pub struct SystemMonitor {
    pub active_tab: MonitorTab,
    pub processes: Vec<ProcessInfo>,
    pub cpu_history: RingBuffer<CpuSnapshot>,
    pub memory_info: MemoryInfo,
    pub gpu_info: GpuMonitorInfo,
    pub disk_info: Vec<DiskInfo>,
    pub network_info: Vec<NetworkInterfaceInfo>,
    pub sort_column: ProcessColumn,
    pub sort_ascending: bool,
    pub selected_process: Option<u64>,
    pub update_interval_ms: u64,
    pub search_query: String,
    pub graph_width: usize,
}

impl SystemMonitor {
    pub fn new() -> Self {
        Self {
            active_tab: MonitorTab::Overview,
            processes: Vec::new(),
            cpu_history: RingBuffer::new(120),
            memory_info: MemoryInfo {
                total: 0,
                used: 0,
                cached: 0,
                buffers: 0,
                swap_total: 0,
                swap_used: 0,
            },
            gpu_info: GpuMonitorInfo {
                name: String::from("Unknown GPU"),
                usage_percent: 0.0,
                vram_used: 0,
                vram_total: 0,
                temp_c: 0.0,
                power_w: 0.0,
                clock_mhz: 0,
                fan_percent: 0,
            },
            disk_info: Vec::new(),
            network_info: Vec::new(),
            sort_column: ProcessColumn::Cpu,
            sort_ascending: false,
            selected_process: None,
            update_interval_ms: 1000,
            search_query: String::new(),
            graph_width: 80,
        }
    }

    pub fn update(
        &mut self,
        cpu: CpuSnapshot,
        mem: MemoryInfo,
        gpu: GpuMonitorInfo,
        disks: Vec<DiskInfo>,
        nets: Vec<NetworkInterfaceInfo>,
        procs: Vec<ProcessInfo>,
    ) {
        self.cpu_history.push(cpu);
        self.memory_info = mem;
        self.gpu_info = gpu;
        self.disk_info = disks;
        self.network_info = nets;
        self.processes = procs;
        self.sort_processes();
    }

    pub fn kill_process(&mut self, pid: u64) -> bool {
        if let Some(pos) = self.processes.iter().position(|p| p.pid == pid) {
            self.processes.remove(pos);
            if self.selected_process == Some(pid) {
                self.selected_process = None;
            }
            true
        } else {
            false
        }
    }

    pub fn set_priority(&mut self, pid: u64, priority: &str) {
        if let Some(proc) = self.processes.iter_mut().find(|p| p.pid == pid) {
            proc.priority = String::from(priority);
        }
    }

    pub fn sort_processes(&mut self) {
        let asc = self.sort_ascending;
        let col = self.sort_column;
        self.processes.sort_by(|a, b| {
            let ord = match col {
                ProcessColumn::Pid => a.pid.cmp(&b.pid),
                ProcessColumn::Name => a.name.cmp(&b.name),
                ProcessColumn::Cpu => a
                    .cpu_percent
                    .partial_cmp(&b.cpu_percent)
                    .unwrap_or(core::cmp::Ordering::Equal),
                ProcessColumn::Memory => a.memory_bytes.cmp(&b.memory_bytes),
                ProcessColumn::Threads => a.threads.cmp(&b.threads),
                ProcessColumn::DiskRead => a.disk_read_bytes.cmp(&b.disk_read_bytes),
                ProcessColumn::DiskWrite => a.disk_write_bytes.cmp(&b.disk_write_bytes),
                ProcessColumn::User => a.user.cmp(&b.user),
                ProcessColumn::Priority => a.priority.cmp(&b.priority),
            };
            if asc {
                ord
            } else {
                ord.reverse()
            }
        });
    }

    pub fn search_processes(&self) -> Vec<&ProcessInfo> {
        if self.search_query.is_empty() {
            self.processes.iter().collect()
        } else {
            self.processes
                .iter()
                .filter(|p| {
                    p.name.contains(&self.search_query)
                        || p.command_line.contains(&self.search_query)
                })
                .collect()
        }
    }

    // ── Render ───────────────────────────────────────────────────────

    pub fn render(&self, canvas: &mut raegfx::Canvas, x: usize, y: usize, w: usize, h: usize) {
        canvas.fill_rect(x, y, w, h, MON_BG);
        canvas.draw_rect_outline(x, y, w, h, MON_BORDER);

        // Tab bar
        let tab_h = 28usize;
        canvas.fill_rect(x, y, w, tab_h, MON_HEADER_BG);
        let tabs = [
            (MonitorTab::Overview, "Overview"),
            (MonitorTab::Processes, "Processes"),
            (MonitorTab::Performance, "Performance"),
            (MonitorTab::Disks, "Disks"),
            (MonitorTab::Network, "Network"),
            (MonitorTab::Gpu, "GPU"),
        ];
        let mut tx = x + 8;
        for (tab, label) in &tabs {
            let label_w = label.len() * GLYPH_W + 16;
            if *tab == self.active_tab {
                canvas.fill_rect(tx, y + 1, label_w, tab_h - 1, MON_SELECTED);
                canvas.draw_text(tx + 8, y + 10, label, MON_ACCENT, None);
            } else {
                canvas.draw_text(tx + 8, y + 10, label, MON_DIM, None);
            }
            tx += label_w + 4;
        }

        let content_y = y + tab_h;
        let content_h = h.saturating_sub(tab_h);

        match self.active_tab {
            MonitorTab::Overview => self.render_overview(canvas, x, content_y, w, content_h),
            MonitorTab::Processes => self.render_process_list(canvas, x, content_y, w, content_h),
            MonitorTab::Performance => self.render_performance(canvas, x, content_y, w, content_h),
            MonitorTab::Disks => self.render_disk_view(canvas, x, content_y, w, content_h),
            MonitorTab::Network => self.render_network_graph(canvas, x, content_y, w, content_h),
            MonitorTab::Gpu => self.render_gpu_graph(canvas, x, content_y, w, content_h),
        }
    }

    fn render_overview(&self, canvas: &mut raegfx::Canvas, x: usize, y: usize, w: usize, h: usize) {
        let panel_h = h / 3;

        // CPU summary
        canvas.fill_rect(x + 8, y + 8, w / 2 - 16, panel_h - 16, MON_PANEL_BG);
        canvas.draw_text(x + 16, y + 16, "CPU", MON_ACCENT, None);
        if let Some(snap) = self.cpu_history.latest() {
            let cpu_str = format!("{:.1}%", snap.total);
            canvas.draw_text(x + 16, y + 32, &cpu_str, MON_FG, None);
            let freq_str = format!("{} MHz", snap.frequency_mhz);
            canvas.draw_text(x + 16, y + 48, &freq_str, MON_DIM, None);
            if let Some(temp) = snap.temp_c {
                let temp_str = format!("{:.0}C", temp);
                let color = if temp > 80.0 {
                    MON_RED
                } else if temp > 65.0 {
                    MON_YELLOW
                } else {
                    MON_GREEN
                };
                canvas.draw_text(x + 16, y + 64, &temp_str, color, None);
            }
            self.render_cpu_graph(canvas, x + 100, y + 16, w / 2 - 130, panel_h - 32);
        }

        // Memory summary
        canvas.fill_rect(x + w / 2 + 8, y + 8, w / 2 - 16, panel_h - 16, MON_PANEL_BG);
        canvas.draw_text(x + w / 2 + 16, y + 16, "Memory", MON_PURPLE, None);
        self.render_memory_graph(canvas, x + w / 2 + 16, y + 32, w / 2 - 40, panel_h - 48);

        // GPU summary
        canvas.fill_rect(
            x + 8,
            y + panel_h + 8,
            w / 2 - 16,
            panel_h - 16,
            MON_PANEL_BG,
        );
        canvas.draw_text(x + 16, y + panel_h + 16, "GPU", MON_CYAN, None);
        let gpu_str = format!(
            "{:.0}% — {}C",
            self.gpu_info.usage_percent, self.gpu_info.temp_c as u32
        );
        canvas.draw_text(x + 16, y + panel_h + 32, &gpu_str, MON_FG, None);
        let vram_str = format!(
            "VRAM: {} / {}",
            format_bytes(self.gpu_info.vram_used),
            format_bytes(self.gpu_info.vram_total)
        );
        canvas.draw_text(x + 16, y + panel_h + 48, &vram_str, MON_DIM, None);

        // Network summary
        canvas.fill_rect(
            x + w / 2 + 8,
            y + panel_h + 8,
            w / 2 - 16,
            panel_h - 16,
            MON_PANEL_BG,
        );
        canvas.draw_text(x + w / 2 + 16, y + panel_h + 16, "Network", MON_GREEN, None);
        for (i, net) in self.network_info.iter().take(3).enumerate() {
            let ny = y + panel_h + 32 + i * 16;
            let status = if net.connected { "UP" } else { "DN" };
            let net_str = format!(
                "{}: {} | rx {} tx {}",
                net.name,
                status,
                format_rate(net.rx_bps),
                format_rate(net.tx_bps)
            );
            canvas.draw_text(x + w / 2 + 16, ny, &net_str, MON_DIM, None);
        }
    }

    pub fn render_process_list(
        &self,
        canvas: &mut raegfx::Canvas,
        x: usize,
        y: usize,
        w: usize,
        h: usize,
    ) {
        let row_h = 18usize;

        // Column headers
        canvas.fill_rect(x, y, w, row_h, MON_HEADER_BG);
        let cols: [(ProcessColumn, &str, usize); 7] = [
            (ProcessColumn::Pid, "PID", 60),
            (ProcessColumn::Name, "Name", 160),
            (ProcessColumn::Cpu, "CPU%", 60),
            (ProcessColumn::Memory, "Memory", 80),
            (ProcessColumn::Threads, "Threads", 60),
            (ProcessColumn::User, "User", 80),
            (ProcessColumn::Priority, "Priority", 70),
        ];
        let mut cx = x + 8;
        for (col, label, col_w) in &cols {
            let fg = if *col == self.sort_column {
                MON_ACCENT
            } else {
                MON_DIM
            };
            canvas.draw_text(cx, y + 5, label, fg, None);
            cx += col_w;
        }

        // Process rows
        let procs = self.search_processes();
        let max_rows = h.saturating_sub(row_h) / row_h;
        for (i, proc) in procs.iter().take(max_rows).enumerate() {
            let ry = y + row_h + i * row_h;
            if Some(proc.pid) == self.selected_process {
                canvas.fill_rect(x, ry, w, row_h, MON_SELECTED);
            }

            let mut cx = x + 8;
            let pid_str = format!("{}", proc.pid);
            canvas.draw_text(cx, ry + 5, &pid_str, MON_DIM, None);
            cx += 60;

            let max_name = 18;
            let name = crate::text_util::truncate_chars(&proc.name, max_name);
            canvas.draw_text(cx, ry + 5, name, MON_FG, None);
            cx += 160;

            let cpu_str = format!("{:.1}", proc.cpu_percent);
            let cpu_color = if proc.cpu_percent > 80.0 {
                MON_RED
            } else if proc.cpu_percent > 50.0 {
                MON_YELLOW
            } else {
                MON_GREEN
            };
            canvas.draw_text(cx, ry + 5, &cpu_str, cpu_color, None);
            cx += 60;

            let mem_str = format_bytes(proc.memory_bytes);
            canvas.draw_text(cx, ry + 5, &mem_str, MON_DIM, None);
            cx += 80;

            let thr_str = format!("{}", proc.threads);
            canvas.draw_text(cx, ry + 5, &thr_str, MON_DIM, None);
            cx += 60;

            let max_user = 8;
            let user = crate::text_util::truncate_chars(&proc.user, max_user);
            canvas.draw_text(cx, ry + 5, user, MON_DIM, None);
            cx += 80;

            canvas.draw_text(cx, ry + 5, &proc.priority, MON_DIM, None);
        }
    }

    fn render_performance(
        &self,
        canvas: &mut raegfx::Canvas,
        x: usize,
        y: usize,
        w: usize,
        h: usize,
    ) {
        let half_h = h / 2;
        canvas.fill_rect(x + 8, y + 8, w - 16, half_h - 16, MON_PANEL_BG);
        canvas.draw_text(x + 16, y + 16, "CPU History", MON_ACCENT, None);
        self.render_cpu_graph(canvas, x + 16, y + 32, w - 40, half_h - 48);

        canvas.fill_rect(x + 8, y + half_h + 8, w - 16, half_h - 16, MON_PANEL_BG);
        canvas.draw_text(x + 16, y + half_h + 16, "Memory", MON_PURPLE, None);
        self.render_memory_graph(canvas, x + 16, y + half_h + 32, w - 40, half_h - 48);
    }

    pub fn render_cpu_graph(
        &self,
        canvas: &mut raegfx::Canvas,
        x: usize,
        y: usize,
        w: usize,
        h: usize,
    ) {
        canvas.fill_rect(x, y, w, h, MON_GRAPH_BG);
        canvas.draw_rect_outline(x, y, w, h, MON_BORDER);

        // Grid lines at 25%, 50%, 75%
        for pct in &[25, 50, 75] {
            let gy = y + h - (*pct as usize * h / 100);
            for gx in (x..x + w).step_by(4) {
                canvas.draw_pixel(gx, gy, MON_BORDER);
            }
        }

        let snapshots: Vec<&CpuSnapshot> = self.cpu_history.iter_chronological().collect();
        if snapshots.len() < 2 {
            return;
        }

        let point_count = snapshots.len().min(w);
        let step = if snapshots.len() > w {
            snapshots.len() / w
        } else {
            1
        };

        for i in 1..point_count {
            let idx0 = (i - 1) * step;
            let idx1 = i * step;
            if idx0 >= snapshots.len() || idx1 >= snapshots.len() {
                break;
            }

            let v0 = snapshots[idx0].total.clamp(0.0, 100.0);
            let v1 = snapshots[idx1].total.clamp(0.0, 100.0);
            let x0 = x + (i - 1) * w / point_count.max(1);
            let x1 = x + i * w / point_count.max(1);
            let y0 = y + h - (v0 as usize * h / 100);
            let y1 = y + h - (v1 as usize * h / 100);
            canvas.draw_line(x0 as i32, y0 as i32, x1 as i32, y1 as i32, MON_ACCENT);
        }
    }

    pub fn render_memory_graph(
        &self,
        canvas: &mut raegfx::Canvas,
        x: usize,
        y: usize,
        w: usize,
        h: usize,
    ) {
        let mem = &self.memory_info;
        if mem.total == 0 {
            return;
        }

        let used_pct = (mem.used as f64 / mem.total as f64 * 100.0) as usize;
        let cached_pct = (mem.cached as f64 / mem.total as f64 * 100.0) as usize;
        let bar_h = 20usize.min(h / 3);

        // Used bar
        canvas.fill_rect(x, y, w, bar_h, MON_GRAPH_BG);
        let used_w = used_pct * w / 100;
        let color = if used_pct > 90 {
            MON_RED
        } else if used_pct > 70 {
            MON_YELLOW
        } else {
            MON_PURPLE
        };
        canvas.fill_rect(x, y, used_w, bar_h, color);

        // Cached overlay (lighter shade on top)
        let cached_w = cached_pct * w / 100;
        canvas.fill_rect(x + used_w, y, cached_w.min(w - used_w), bar_h, MON_BORDER);

        // Labels
        let used_str = format!(
            "Used: {} / {} ({}%)",
            format_bytes(mem.used),
            format_bytes(mem.total),
            used_pct
        );
        canvas.draw_text(x, y + bar_h + 4, &used_str, MON_FG, None);

        let cache_str = format!(
            "Cached: {} | Buffers: {}",
            format_bytes(mem.cached),
            format_bytes(mem.buffers)
        );
        canvas.draw_text(x, y + bar_h + 18, &cache_str, MON_DIM, None);

        if mem.swap_total > 0 {
            let swap_str = format!(
                "Swap: {} / {}",
                format_bytes(mem.swap_used),
                format_bytes(mem.swap_total)
            );
            canvas.draw_text(x, y + bar_h + 32, &swap_str, MON_DIM, None);
        }
    }

    pub fn render_gpu_graph(
        &self,
        canvas: &mut raegfx::Canvas,
        x: usize,
        y: usize,
        w: usize,
        h: usize,
    ) {
        canvas.fill_rect(x + 8, y + 8, w - 16, h - 16, MON_PANEL_BG);
        canvas.draw_text(x + 16, y + 16, &self.gpu_info.name, MON_CYAN, None);

        let bar_y = y + 36;
        let bar_w = w - 40;
        let bar_h = 16;

        // Usage bar
        canvas.draw_text(x + 16, bar_y, "Usage", MON_DIM, None);
        canvas.fill_rect(x + 80, bar_y, bar_w, bar_h, MON_GRAPH_BG);
        let usage_w = (self.gpu_info.usage_percent as usize * bar_w) / 100;
        canvas.fill_rect(x + 80, bar_y, usage_w, bar_h, MON_CYAN);

        // VRAM bar
        let vram_y = bar_y + bar_h + 8;
        canvas.draw_text(x + 16, vram_y, "VRAM", MON_DIM, None);
        canvas.fill_rect(x + 80, vram_y, bar_w, bar_h, MON_GRAPH_BG);
        if self.gpu_info.vram_total > 0 {
            let vram_w =
                (self.gpu_info.vram_used as usize * bar_w) / self.gpu_info.vram_total as usize;
            canvas.fill_rect(x + 80, vram_y, vram_w, bar_h, MON_PURPLE);
        }

        // Stats
        let stats_y = vram_y + bar_h + 16;
        let temp_color = if self.gpu_info.temp_c > 85.0 {
            MON_RED
        } else if self.gpu_info.temp_c > 70.0 {
            MON_YELLOW
        } else {
            MON_GREEN
        };
        let temp_str = format!("Temp: {:.0}C", self.gpu_info.temp_c);
        canvas.draw_text(x + 16, stats_y, &temp_str, temp_color, None);

        let power_str = format!("Power: {:.1}W", self.gpu_info.power_w);
        canvas.draw_text(x + 16, stats_y + 16, &power_str, MON_DIM, None);

        let clock_str = format!("Clock: {} MHz", self.gpu_info.clock_mhz);
        canvas.draw_text(x + 16, stats_y + 32, &clock_str, MON_DIM, None);

        let fan_str = format!("Fan: {}%", self.gpu_info.fan_percent);
        canvas.draw_text(x + 16, stats_y + 48, &fan_str, MON_DIM, None);

        let vram_str = format!(
            "VRAM: {} / {}",
            format_bytes(self.gpu_info.vram_used),
            format_bytes(self.gpu_info.vram_total)
        );
        canvas.draw_text(x + 16, stats_y + 64, &vram_str, MON_DIM, None);
    }

    pub fn render_disk_view(
        &self,
        canvas: &mut raegfx::Canvas,
        x: usize,
        y: usize,
        w: usize,
        h: usize,
    ) {
        let panel_h = 70usize;
        for (i, disk) in self.disk_info.iter().enumerate() {
            let dy = y + 8 + i * (panel_h + 8);
            if dy + panel_h > y + h {
                break;
            }

            canvas.fill_rect(x + 8, dy, w - 16, panel_h, MON_PANEL_BG);
            let header = format!("{} ({})", disk.name, disk.mount);
            canvas.draw_text(x + 16, dy + 8, &header, MON_FG, None);

            // Usage bar
            let bar_y = dy + 24;
            let bar_w = w - 40;
            let bar_h = 14;
            canvas.fill_rect(x + 16, bar_y, bar_w, bar_h, MON_GRAPH_BG);
            if disk.total > 0 {
                let used_pct = (disk.used as usize * 100) / disk.total as usize;
                let used_w = used_pct * bar_w / 100;
                let color = if used_pct > 90 {
                    MON_RED
                } else if used_pct > 75 {
                    MON_YELLOW
                } else {
                    MON_GREEN
                };
                canvas.fill_rect(x + 16, bar_y, used_w, bar_h, color);
            }

            let usage_str = format!(
                "{} / {} — R: {} W: {}",
                format_bytes(disk.used),
                format_bytes(disk.total),
                format_rate(disk.read_bps),
                format_rate(disk.write_bps)
            );
            canvas.draw_text(x + 16, bar_y + bar_h + 4, &usage_str, MON_DIM, None);

            if let Some(temp) = disk.temp_c {
                let temp_str = format!("{}C", temp);
                canvas.draw_text(x + w - 60, dy + 8, &temp_str, MON_DIM, None);
            }
        }
    }

    pub fn render_network_graph(
        &self,
        canvas: &mut raegfx::Canvas,
        x: usize,
        y: usize,
        w: usize,
        h: usize,
    ) {
        let panel_h = 60usize;
        for (i, net) in self.network_info.iter().enumerate() {
            let ny = y + 8 + i * (panel_h + 8);
            if ny + panel_h > y + h {
                break;
            }

            canvas.fill_rect(x + 8, ny, w - 16, panel_h, MON_PANEL_BG);

            let status = if net.connected {
                "Connected"
            } else {
                "Disconnected"
            };
            let status_color = if net.connected { MON_GREEN } else { MON_RED };
            canvas.draw_text(x + 16, ny + 8, &net.name, MON_FG, None);
            canvas.draw_text(
                x + 16 + (net.name.len() + 1) * GLYPH_W,
                ny + 8,
                status,
                status_color,
                None,
            );
            canvas.draw_text(x + 16, ny + 22, &net.ip, MON_DIM, None);

            let rx_str = format!(
                "RX: {} ({} total)",
                format_rate(net.rx_bps),
                format_bytes(net.rx_total)
            );
            canvas.draw_text(x + 16, ny + 36, &rx_str, MON_GREEN, None);

            let tx_str = format!(
                "TX: {} ({} total)",
                format_rate(net.tx_bps),
                format_bytes(net.tx_total)
            );
            let tx_x = x + w / 2;
            canvas.draw_text(tx_x, ny + 36, &tx_str, MON_ORANGE, None);
        }
    }
}

// ── Helpers ──────────────────────────────────────────────────────────────

pub fn format_bytes(bytes: u64) -> String {
    if bytes < 1024 {
        format!("{} B", bytes)
    } else if bytes < 1024 * 1024 {
        format!("{}.{} KB", bytes / 1024, (bytes % 1024) * 10 / 1024)
    } else if bytes < 1024 * 1024 * 1024 {
        let mb = bytes / (1024 * 1024);
        let frac = (bytes % (1024 * 1024)) * 10 / (1024 * 1024);
        format!("{}.{} MB", mb, frac)
    } else {
        let gb = bytes / (1024 * 1024 * 1024);
        let frac = (bytes % (1024 * 1024 * 1024)) * 10 / (1024 * 1024 * 1024);
        format!("{}.{} GB", gb, frac)
    }
}

pub fn format_rate(bytes_per_sec: u64) -> String {
    if bytes_per_sec < 1024 {
        format!("{} B/s", bytes_per_sec)
    } else if bytes_per_sec < 1024 * 1024 {
        format!(
            "{}.{} KB/s",
            bytes_per_sec / 1024,
            (bytes_per_sec % 1024) * 10 / 1024
        )
    } else {
        let mb = bytes_per_sec / (1024 * 1024);
        let frac = (bytes_per_sec % (1024 * 1024)) * 10 / (1024 * 1024);
        format!("{}.{} MB/s", mb, frac)
    }
}
