// Local validation added by ARM RSTiny; the parser APIs assume a valid tree.
use crate::parsing::{CStr, FdtData};

pub(crate) fn blob(data: &[u8]) -> Option<()> {
    let word = |offset| -> Option<usize> {
        Some(u32::from_be_bytes(data.get(offset..offset + 4)?.try_into().ok()?) as usize)
    };
    if data.len() < 40 || word(20)? < 17 || word(24)? > 17 {
        return None;
    }
    let block = |offset, size| -> Option<core::ops::Range<usize>> {
        let start = word(offset)?;
        let end = start.checked_add(word(size)?)?;
        if start < 40 || end > data.len() {
            return None;
        }
        Some(start..end)
    };
    let structure = block(8, 36)?;
    let strings = block(12, 32)?;
    if structure.start % 4 != 0
        || structure.len() % 4 != 0
        || (structure.start < strings.end && strings.start < structure.end)
    {
        return None;
    }
    let reserve = word(16)?;
    if reserve < 40 || reserve % 8 != 0 {
        return None;
    }
    let mut end = reserve;
    loop {
        let next = end.checked_add(16)?;
        let entry = data.get(end..next)?;
        end = next;
        if entry.iter().all(|b| *b == 0) {
            break;
        }
    }
    if [structure.clone(), strings.clone()]
        .iter()
        .any(|block| reserve < block.end && block.start < end)
    {
        return None;
    }

    let strings = &data[strings];
    let mut stream = FdtData::new(&data[structure]);
    let mut depth = 0usize;
    let mut root_seen = false;
    // Existing traversal keeps a fixed-size parent stack indexed from one.
    let mut children_started = [false; 64];
    while let Some(token) = stream.u32() {
        match token.get() {
            1 => {
                if depth == 0 && root_seen || depth >= 63 {
                    return None;
                }
                children_started[depth] = true;
                let name = CStr::new(stream.remaining())?.as_str()?;
                if depth == 0 && !name.is_empty() {
                    return None;
                }
                stream.take((name.len() + 1).next_multiple_of(4))?;
                depth += 1;
                children_started[depth] = false;
                root_seen = true;
            }
            2 => {
                depth = depth.checked_sub(1)?;
            }
            3 => {
                if depth == 0 || children_started[depth] {
                    return None;
                }
                let len = stream.u32()?.get() as usize;
                let name_offset = stream.u32()?.get() as usize;
                CStr::new(strings.get(name_offset..)?)?.as_str()?;
                let aligned = len.checked_add(3)? & !3;
                stream.take(aligned)?;
            }
            4 => {}
            9 if root_seen && depth == 0 => return Some(()),
            _ => return None,
        }
    }
    None
}
