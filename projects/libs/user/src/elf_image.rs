//! Borrowed ELF64 program headers; no segment arrays on the small user stack.
const PAGE: usize = 4096;
#[derive(Clone, Copy)]
pub struct Segment {
    pub flags: usize,
    pub offset: usize,
    pub va: usize,
    pub filesz: usize,
    pub memsz: usize,
    pub end: usize,
}
pub struct Elf<'a> {
    headers: &'a [u8],
    pub start: usize,
    pub end: usize,
    pub entry: usize,
}
fn integer(data: &[u8], offset: usize, size: usize) -> Result<usize, ()> {
    let data = data
        .get(offset..offset.checked_add(size).ok_or(())?)
        .ok_or(())?;
    Ok(data
        .iter()
        .enumerate()
        .fold(0, |n, (i, byte)| n | ((*byte as usize) << (i * 8))))
}
fn segment(header: &[u8]) -> Result<Segment, ()> {
    let va = integer(header, 16, 8)?;
    let memsz = integer(header, 40, 8)?;
    Ok(Segment {
        flags: integer(header, 4, 4)?,
        offset: integer(header, 8, 8)?,
        va,
        filesz: integer(header, 32, 8)?,
        memsz,
        end: va
            .checked_add(memsz)
            .and_then(|n| n.checked_add(PAGE - 1))
            .ok_or(())?
            & !(PAGE - 1),
    })
}
impl<'a> Elf<'a> {
    pub fn parse(bytes: &'a [u8]) -> Result<Self, ()> {
        if bytes.get(..7) != Some(b"\x7fELF\x02\x01\x01")
            || integer(bytes, 16, 2)? != 2
            || integer(bytes, 18, 2)? != 183
            || integer(bytes, 20, 4)? != 1
            || integer(bytes, 52, 2)? != 64
            || integer(bytes, 54, 2)? != 56
        {
            return Err(());
        }
        let count = integer(bytes, 56, 2)?;
        if !(1..=32).contains(&count) {
            return Err(());
        }
        let phoff = integer(bytes, 32, 8)?;
        let headers = bytes
            .get(phoff..phoff.checked_add(count * 56).ok_or(())?)
            .ok_or(())?;
        let mut elf = Self {
            headers,
            start: usize::MAX,
            end: 0,
            entry: integer(bytes, 24, 8)?,
        };
        let mut entry_valid = false;
        for (index, header) in headers.as_chunks::<56>().0.iter().enumerate() {
            let kind = integer(header, 0, 4)?;
            if matches!(kind, 2 | 3 | 7) {
                return Err(());
            }
            if kind != 1 || integer(header, 40, 8)? == 0 {
                continue;
            }
            let s = segment(header)?;
            let align = integer(header, 48, 8)?;
            if !matches!(s.flags, 4..=6)
                || s.filesz > s.memsz
                || s.offset
                    .checked_add(s.filesz)
                    .is_none_or(|end| end > bytes.len())
                || !s.va.is_multiple_of(PAGE)
                || !s.offset.is_multiple_of(PAGE)
                || align < PAGE
                || !align.is_power_of_two()
                || s.va % align != s.offset % align
                || s.va < 0x400000
                || s.end > 0x8000000
            {
                return Err(());
            }
            for previous in headers[..index * 56].as_chunks::<56>().0.iter() {
                if integer(previous, 0, 4)? == 1 && integer(previous, 40, 8)? != 0 {
                    let other = segment(previous)?;
                    if s.va < other.end && other.va < s.end {
                        return Err(());
                    }
                }
            }
            elf.start = elf.start.min(s.va);
            elf.end = elf.end.max(s.end);
            entry_valid |= s.flags == 5 && s.va <= elf.entry && elf.entry < s.va + s.filesz;
        }
        if !entry_valid || !elf.entry.is_multiple_of(4) {
            return Err(());
        }
        Ok(elf)
    }
    pub fn segments(&self) -> impl Iterator<Item = Segment> + '_ {
        self.headers
            .as_chunks::<56>()
            .0
            .iter()
            .filter(|h| integer(*h, 0, 4) == Ok(1) && integer(*h, 40, 8) != Ok(0))
            .map(|h| segment(h).expect("validated ELF header"))
    }
}
