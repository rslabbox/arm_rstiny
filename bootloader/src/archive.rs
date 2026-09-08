//! Bounded newc archive reader. The image contract has exactly three files.
pub fn files(data: &[u8]) -> Result<[&[u8]; 3], &'static str> {
    let mut result = [&[][..]; 3];
    let names: [&[u8]; 3] = [b"kernel.elf", b"kernel.dtb", b"rootserver"];
    let mut cursor = 0usize;
    for index in 0..=3 {
        let header = data
            .get(cursor..cursor.checked_add(110).ok_or("CPIO overflow")?)
            .ok_or("truncated CPIO header")?;
        if &header[..6] != b"070701" {
            return Err("expected newc CPIO");
        }
        let size = hex(&header[54..62])?;
        let namesize = hex(&header[94..102])?;
        cursor += 110;
        let end = cursor.checked_add(namesize).ok_or("CPIO name overflow")?;
        let name = data.get(cursor..end).ok_or("truncated CPIO name")?;
        if name.last() != Some(&0) {
            return Err("unterminated CPIO name");
        }
        cursor = end.checked_add(3).ok_or("CPIO alignment overflow")? & !3;
        let end = cursor.checked_add(size).ok_or("CPIO file overflow")?;
        let file = data.get(cursor..end).ok_or("truncated CPIO file")?;
        if index == 3 {
            if name != b"TRAILER!!!\0" || size != 0 {
                return Err("unexpected CPIO entry");
            }
            return Ok(result);
        }
        if &name[..name.len() - 1] != names[index] {
            return Err("unexpected CPIO file order");
        }
        result[index] = file;
        cursor = end.checked_add(3).ok_or("CPIO alignment overflow")? & !3;
    }
    Err("missing CPIO trailer")
}

fn hex(bytes: &[u8]) -> Result<usize, &'static str> {
    bytes.iter().try_fold(0usize, |value, byte| {
        let digit = match byte {
            b'0'..=b'9' => byte - b'0',
            b'a'..=b'f' => byte - b'a' + 10,
            b'A'..=b'F' => byte - b'A' + 10,
            _ => return Err("invalid CPIO hexadecimal field"),
        };
        value
            .checked_mul(16)
            .and_then(|n| n.checked_add(digit as usize))
            .ok_or("CPIO integer overflow")
    })
}
