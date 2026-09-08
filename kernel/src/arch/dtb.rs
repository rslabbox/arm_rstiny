//! Read the PSCI conduit from the DTB handed over by seL4 elfloader.
pub fn psci_smc(data: &[u8]) -> Option<bool> {
    fn word(data: &[u8], offset: usize) -> Option<usize> {
        Some(
            u32::from_be_bytes(data.get(offset..offset.checked_add(4)?)?.try_into().ok()?) as usize,
        )
    }
    fn string(data: &[u8], offset: usize) -> Option<&[u8]> {
        let rest = data.get(offset..)?;
        Some(&rest[..rest.iter().position(|byte| *byte == 0)?])
    }
    if word(data, 0)? != 0xd00dfeed || word(data, 4)? != data.len() {
        return None;
    }
    let start = word(data, 8)?;
    let structure = data.get(start..start.checked_add(word(data, 36)?)?)?;
    let start = word(data, 12)?;
    let strings = data.get(start..start.checked_add(word(data, 32)?)?)?;
    let mut cursor = 0usize;
    let mut depth = 0usize;
    let mut psci_depth = None;
    while cursor < structure.len() {
        let token = word(structure, cursor)?;
        cursor += 4;
        match token {
            1 => {
                let name = string(structure, cursor)?;
                depth += 1;
                if name == b"psci" || name.starts_with(b"psci@") {
                    psci_depth = Some(depth);
                }
                cursor = (cursor + name.len() + 1).next_multiple_of(4);
            }
            2 => {
                if psci_depth == Some(depth) {
                    psci_depth = None;
                }
                depth = depth.checked_sub(1)?;
            }
            3 => {
                let len = word(structure, cursor)?;
                let name = string(strings, word(structure, cursor + 4)?)?;
                cursor += 8;
                let value = structure.get(cursor..cursor.checked_add(len)?)?;
                if psci_depth == Some(depth) && name == b"method" {
                    return match value {
                        b"smc\0" => Some(true),
                        b"hvc\0" => Some(false),
                        _ => None,
                    };
                }
                cursor = (cursor + len).next_multiple_of(4);
            }
            4 => {}
            9 => break,
            _ => return None,
        }
    }
    None
}
