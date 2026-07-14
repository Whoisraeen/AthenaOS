pub struct TarArchive<'a> {
    data: &'a [u8],
}

pub struct TarFile<'a> {
    pub name: &'a str,
    pub data: &'a [u8],
}

impl<'a> TarArchive<'a> {
    pub fn new(data: &'a [u8]) -> Self {
        TarArchive { data }
    }

    pub fn get_file(&self, path: &str) -> Option<TarFile<'a>> {
        let mut offset = 0;
        while offset + 512 <= self.data.len() {
            let header = &self.data[offset..offset + 512];
            if header[0] == 0 {
                break; // End of archive
            }

            // Read name (100 bytes, null-terminated)
            let mut name_len = 0;
            while name_len < 100 && header[name_len] != 0 {
                name_len += 1;
            }
            let name = core::str::from_utf8(&header[0..name_len]).unwrap_or("");

            // Read size (octal ascii, 12 bytes at offset 124)
            let size_bytes = &header[124..135];
            let size_str = core::str::from_utf8(size_bytes).unwrap_or("0").trim();
            let size = usize::from_str_radix(size_str, 8).unwrap_or(0);

            let file_data_start = offset + 512;
            let file_data_end = file_data_start + size;

            if name == path {
                return Some(TarFile {
                    name,
                    data: &self.data[file_data_start..file_data_end],
                });
            }

            // Advance offset (size rounded up to 512)
            offset = file_data_start + ((size + 511) & !511);
        }
        None
    }

    pub fn iter(&'a self) -> TarIter<'a> {
        TarIter {
            archive: self,
            offset: 0,
        }
    }
}

pub struct TarIter<'a> {
    archive: &'a TarArchive<'a>,
    offset: usize,
}

impl<'a> Iterator for TarIter<'a> {
    type Item = TarFile<'a>;

    fn next(&mut self) -> Option<Self::Item> {
        while self.offset + 512 <= self.archive.data.len() {
            let header = &self.archive.data[self.offset..self.offset + 512];
            if header[0] == 0 {
                return None; // End of archive
            }

            let mut name_len = 0;
            while name_len < 100 && header[name_len] != 0 {
                name_len += 1;
            }
            let name = core::str::from_utf8(&header[0..name_len]).unwrap_or("");

            let size_bytes = &header[124..135];
            let size_str = core::str::from_utf8(size_bytes).unwrap_or("0").trim();
            let size = usize::from_str_radix(size_str, 8).unwrap_or(0);

            let file_data_start = self.offset + 512;
            let file_data_end = file_data_start + size;

            // A TRUNCATED archive (the iron/UEFI initramfs can be cut off near a
            // size boundary) would make this slice run past the end and PANIC the
            // kernel mid-boot. Stop enumeration at the cut instead — every entry
            // BEFORE the truncation still parses, and a missing late entry
            // degrades to a spawn/open ENOENT, not a halt. FAIL-able: the WARN
            // line names the first truncated entry so the cause is visible.
            if file_data_end > self.archive.data.len() {
                crate::serial_println!(
                    "[tar] WARN: entry '{}' truncated (needs {} bytes, archive has {}) -- stopping initramfs enumeration here",
                    name,
                    file_data_end,
                    self.archive.data.len()
                );
                return None;
            }

            let file = TarFile {
                name,
                data: &self.archive.data[file_data_start..file_data_end],
            };

            self.offset = file_data_start + ((size + 511) & !511);

            // Skip directories (size == 0 or name ends with '/')
            if size > 0 && !name.ends_with('/') {
                return Some(file);
            }
        }
        None
    }
}
