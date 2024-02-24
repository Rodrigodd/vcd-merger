use fxhash::FxHashMap as HashMap;
use memmap2::Mmap;
use std::cmp::Reverse;
use std::collections::binary_heap::PeekMut;
use std::io::{BufRead, BufWriter, Write};
use std::sync::Mutex;

// this can only represent 94^4 = 78_074_896 symbols.
#[derive(Clone, Copy, Hash, PartialEq, Eq)]
struct IdCode([u8; 4]);
impl From<&[u8]> for IdCode {
    fn from(s: &[u8]) -> Self {
        let mut code = [0; 4];
        for (i, b) in s.iter().enumerate() {
            code[i] = *b;
        }
        IdCode(code)
    }
}
impl IdCode {
    fn as_bytes(&self) -> &[u8] {
        for i in 0..4 {
            if self.0[i] == 0 {
                return &self.0[..i];
            }
        }
        &self.0
    }
}
impl std::fmt::Debug for IdCode {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "IdCode(")?;
        for b in self.as_bytes() {
            write!(f, "{}", *b as char)?;
        }
        write!(f, ")")?;
        Ok(())
    }
}

struct Vcd {
    /// Map from old symbol to new symbol.
    symbol_map: HashMap<IdCode, IdCode>,
    /// All scope and var declarations.
    declarations: Vec<String>,
    file: Mmap,
    end_of_definitions: usize,
    /// The timescale ratio between this input timescale and the output timescale.
    timescale: u64,
}

#[derive(Default)]
struct Header {
    date: Option<String>,
    version: Option<String>,
    timescale: Option<String>,
}

const PROGRESS_BAR_TEMPLATE: &str =
    "{elapsed_precise} █{bar:60.cyan/blue}█ {bytes}/{total_bytes} {binary_bytes_per_sec} ({eta})";

fn main() {
    let args = std::env::args().collect::<Vec<String>>();

    if args.len() < 3 {
        println!("usage: vcd-merger <input.vcd> [<input.vcd> *] <output.vcd>");
        return;
    }

    let style = indicatif::ProgressStyle::default_bar()
        .template(PROGRESS_BAR_TEMPLATE)
        .unwrap()
        .progress_chars("█▉▊▋▌▍▎▏  ");

    let inputs = &args[1..args.len() - 1];
    let output = &args[args.len() - 1];

    println!("[1/3] gathering symbols");

    let mut headers = Header::default();

    let vcds = parse_headers(inputs, &mut headers);

    println!("[2/3] finding sections");

    let total_len = vcds.iter().map(|vcd| vcd.file.len() as u64).sum::<u64>();
    let bar = indicatif::ProgressBar::new(total_len).with_style(style.clone());
    let on_progress = |progress| bar.set_position(progress);

    let sections = find_sections(&vcds, on_progress);

    bar.finish();

    println!("[3/3] merging {} sections", sections.len());

    let total_len = sections.iter().map(|s| s.section.len() as u64).sum::<u64>();
    let bar = indicatif::ProgressBar::new(total_len).with_style(style);
    let on_progress = |progress| bar.set_position(progress);

    write_output(output, headers, &vcds, sections, on_progress).unwrap();

    bar.finish();
}

fn next_code() -> IdCode {
    static CURR_CODE: Mutex<IdCode> = Mutex::new(IdCode([0; 4])); // '!'
    let mut code = CURR_CODE.lock().unwrap();

    for b in code.0.iter_mut() {
        // '~'
        if *b == 0x0 {
            // '!'
            *b = 0x21;
            break;
        }
        if *b < 0x7E {
            *b += 1;
            break;
        } else {
            // '!'
            *b = 0x21;
        }
    }

    *code
}

fn take_to_end(tokens: &mut impl Iterator<Item = String>) -> String {
    let mut scale = String::with_capacity(8);
    for token in tokens.by_ref() {
        if token == "$end" {
            break;
        }
        scale.push_str(&token);
        scale.push(' ');
    }
    scale
}

fn parse_headers(inputs: &[String], header: &mut Header) -> Vec<Vcd> {
    let mut vcds = Vec::new();
    for input in inputs {
        let file = std::fs::File::open(input).unwrap();
        // let mut reader = BufReader::with_capacity(0x1_0000, file);
        let memmap = unsafe { memmap2::MmapOptions::new().map(&file).unwrap() };
        let mut reader = std::io::Cursor::new(memmap);

        let mut lines = (&mut reader).lines().map_while(Result::ok);

        let mut tokens = lines.by_ref().flat_map(|line| {
            line.split_ascii_whitespace()
                .map(String::from)
                .collect::<Vec<_>>()
        });

        let mut symbol_map = HashMap::default();

        let mut declarations = Vec::new();

        let mut timescale = 0;

        while let Some(token) = tokens.next() {
            match token.as_str() {
                "$date" => {
                    let date = take_to_end(&mut tokens);
                    if header.date.is_none() {
                        header.date = Some(date);
                    }
                }
                "$version" => {
                    let version = take_to_end(&mut tokens);
                    if header.version.is_none() {
                        header.version = Some(version);
                    }
                }
                "$timescale" => {
                    let scale = take_to_end(&mut tokens);

                    // parse .*\d*.*(fs|ps|ns|ms|s)
                    let n = scale
                        .find(|x: char| x.is_ascii_digit())
                        .expect("invalid timestamp");
                    let e = scale[n..]
                        .find(|x: char| !x.is_ascii_digit())
                        .expect("invalid timestamp");
                    let u = scale[n + e..]
                        .find(['f', 'p', 'n', 'm', 's'])
                        .expect("invalid timestamp");

                    let number = parse_u64(scale[n..n + e].as_bytes()).unwrap();
                    let unit = &scale[n + e + u..];
                    timescale = match unit[..2].as_bytes() {
                        b"fs" => number,
                        b"ps" => number * 1_000,
                        b"ns" => number * 1_000_000,
                        b"us" => number * 1_000_000_000,
                        b"ms" => number * 1_000_000_000_000,
                        [b's', _] => number * 1_000_000_000_000_000,
                        _ => panic!("invalid timestamp"),
                    };
                }
                "$scope" => {
                    let module = tokens.next().unwrap();
                    let name = tokens.next().unwrap();
                    let end = tokens.next().unwrap();

                    assert_eq!(end, "$end");

                    declarations.push(format!("$scope {} {} $end\n", module, name));
                }
                "$var" => {
                    let ty = tokens.next().unwrap();
                    let width = tokens.next().unwrap();
                    let old_id = tokens.next().unwrap();
                    let name = take_to_end(&mut tokens);

                    let old_id = IdCode::from(old_id.as_bytes());
                    let new_id = symbol_map.entry(old_id).or_insert_with(next_code);

                    declarations.push(format!(
                        "$var {} {} {} {} $end\n",
                        ty,
                        width,
                        std::str::from_utf8(new_id.as_bytes()).unwrap(),
                        name.trim()
                    ));
                }
                "$upscope" => {
                    let end = tokens.next().unwrap();
                    assert_eq!(end, "$end");
                    declarations.push("$upscope $end\n".to_string());
                }
                "$enddefinitions" => {
                    let end = tokens.next().unwrap();
                    assert_eq!(end, "$end");
                    break;
                }
                "$dumpvars" => {
                    break;
                }
                _ => {
                    break;
                }
            }
        }

        if timescale == 0 {
            panic!("missing timescale");
        }

        let vcd = Vcd {
            symbol_map,
            declarations,
            end_of_definitions: reader.position() as usize,
            file: reader.into_inner(),
            timescale,
        };
        vcds.push(vcd);
    }

    let gcd = vcds
        .iter()
        .map(|vcd| vcd.timescale)
        .fold(vcds[0].timescale, gcd);

    for vcd in &mut vcds {
        vcd.timescale /= gcd;
    }

    let timescale = match gcd {
        x if x % 1_000_000_000_000_000 == 0 => format!("{}s", x / 1_000_000_000_000_000),
        x if x % 1_000_000_000_000 == 0 => format!("{}ms", x / 1_000_000_000_000),
        x if x % 1_000_000_000 == 0 => format!("{}us", x / 1_000_000_000),
        x if x % 1_000_000 == 0 => format!("{}ns", x / 1_000_000),
        x if x % 1_000 == 0 => format!("{}ps", x / 1_000),
        x => format!("{}fs", x),
    };

    println!("timescale set to {}", timescale);

    header.timescale = Some(timescale);

    let symbol_count: usize = vcds.iter().map(|vcd| vcd.symbol_map.len()).sum();

    println!("{} signals found", symbol_count);

    vcds
}

fn gcd(mut n: u64, mut m: u64) -> u64 {
    assert!(n != 0 && m != 0);
    while m != 0 {
        if m < n {
            (m, n) = (n, m);
        }
        m %= n;
    }
    n
}

struct Section<'a> {
    value: u64,
    section: &'a [u8],
    vcd: &'a Vcd,
}
impl<'a> PartialEq for Section<'a> {
    fn eq(&self, other: &Self) -> bool {
        self.value == other.value
    }
}
impl<'a> PartialOrd for Section<'a> {
    fn partial_cmp(&self, other: &Self) -> Option<std::cmp::Ordering> {
        Some(self.cmp(other))
    }
}
impl<'a> Ord for Section<'a> {
    fn cmp(&self, other: &Self) -> std::cmp::Ordering {
        self.value.cmp(&other.value)
    }
}
impl<'a> Eq for Section<'a> {}

fn parse_u64(s: &[u8]) -> Result<u64, ()> {
    let mut value = 0;
    for &b in s {
        if !b.is_ascii_digit() {
            return Err(());
        }
        value = value * 10 + (b - b'0') as u64;
    }
    Ok(value)
}

fn u64_to_bytes(mut value: u64, buffer: &mut [u8; 20]) -> &[u8] {
    let mut i = buffer.len();
    while value > 0 {
        i -= 1;
        buffer[i] = (value % 10) as u8 + b'0';
        value /= 10;
    }
    &buffer[i..]
}

// Find sections of sorted signal changes. These will be merged sorted when written to the output
// file.
fn find_sections(vcds: &[Vcd], mut on_progress: impl FnMut(u64)) -> Vec<Section> {
    let mut sections = Vec::new();

    let mut line_count: usize = 0;
    let mut progress = 0;

    for vcd in vcds {
        let lines = vcd.file[vcd.end_of_definitions..].split(|&b| b == b'\n');
        let mut curr_section = None;

        for line in lines {
            line_count += 1;

            // Same logic as the one described in write_output, but this is 3 times faster
            if line_count % 0xC_0000 == 0 {
                let offset = line.as_ptr() as usize - vcd.file.as_ptr() as usize;
                on_progress(progress + offset as u64);
            }

            if let [b'#', ..] = line {
                let offset = line.as_ptr() as usize - vcd.file.as_ptr() as usize;
                let curr_line_value = parse_u64(&line[1..]).unwrap() * vcd.timescale;

                // if this is the first line, start a new section
                let Some((section_offset, section_value, last_line_value)) = curr_section else {
                    curr_section = Some((offset, curr_line_value, curr_line_value));
                    continue;
                };

                // if out of order, end this section here
                if curr_line_value < last_line_value {
                    let section = &vcd.file[section_offset..offset];

                    sections.push(Section {
                        value: section_value,
                        section,
                        vcd,
                    });

                    curr_section = Some((offset, curr_line_value, curr_line_value));
                } else {
                    curr_section = Some((section_offset, section_value, curr_line_value));
                }
            }
        }

        // add the last section
        if let Some((last_line_offset, last_line_value, _)) = curr_section {
            let section = &vcd.file[last_line_offset..];
            sections.push(Section {
                value: last_line_value,
                section,
                vcd,
            });
        }

        progress += vcd.file.len() as u64;
    }

    sections
}

fn write_output<'a>(
    output: &String,
    headers: Header,
    vcds: &'a [Vcd],
    mut sections: Vec<Section<'a>>,
    mut on_progress: impl FnMut(u64),
) -> std::io::Result<()> {
    let out_file = std::fs::File::create(output).unwrap();
    let mut out_writer = BufWriter::with_capacity(0x1_0000, out_file); // 64KiB

    if let Some(date) = headers.date {
        out_writer.write_all(b"$date ")?;
        out_writer.write_all(date.as_bytes())?;
        out_writer.write_all(b"$end\n")?;
    }
    if let Some(version) = headers.version {
        out_writer.write_all(b"$version ")?;
        out_writer.write_all(version.as_bytes())?;
        out_writer.write_all(b"$end\n")?;
    }
    if let Some(timescale) = headers.timescale {
        out_writer.write_all(b"$timescale ")?;
        out_writer.write_all(timescale.as_bytes())?;
        out_writer.write_all(b"$end\n")?;
    }

    for vcd in vcds.iter() {
        for line in vcd.declarations.iter() {
            out_writer.write_all(line.as_bytes())?;
        }
    }

    out_writer.write_all(b"$enddefinitions $end\n")?;

    let mut heap = std::collections::BinaryHeap::from(
        sections
            .iter()
            .enumerate()
            .map(|(i, s)| Reverse((s.value, i)))
            .collect::<Vec<_>>(),
    );

    let mut progress = 0;
    let mut line_count: usize = 0;

    let mut last_timestamp = None;

    'sections: while let Some(mut heap_entry) = heap.peek_mut() {
        let Reverse((_, index)) = *heap_entry;
        let section = &mut sections[index];
        let mut lines = section.section.split(|x| *x == b'\n');

        // write the timestamp
        if let Some(_line) = lines.next() {
            if last_timestamp != Some(section.value) {
                out_writer.write_all(b"#")?;
                out_writer.write_all(u64_to_bytes(section.value, &mut [0; 20]))?;
                out_writer.write_all(b"\n")?;
            }
            last_timestamp = Some(section.value);
        } else {
            unreachable!("a section always start with a timestamp");
        }

        for line in lines {
            progress += line.len() as u64 + 1;

            line_count += 1;

            // My test file runs at 17 millions lines per second. Thats is about 270 thousands
            // lines every 16ms, around ~2^18 = 4 * 2^16 = 0x4_0000.
            // But I am running this on a SSD, so maybe it is not the best calibration for a HDD
            // user (if the disk is the bottleneck, that is);
            if line_count % 0x4_0000 == 0 {
                on_progress(progress);
            }

            match &line {
                [b'#', ..] => {
                    let offset = line.as_ptr() as usize - section.section.as_ptr() as usize;
                    let value = parse_u64(&line[1..]).unwrap() * section.vcd.timescale;
                    *section = Section {
                        value,
                        section: &section.section[offset..],
                        vcd: section.vcd,
                    };
                    *heap_entry = Reverse((value, index));

                    continue 'sections;
                }
                [b'b', ..] | [b'r', ..] => {
                    let pos = line.iter().position(|c| *c == b' ').unwrap();
                    let (name, symbol) = line.split_at(pos + 1);
                    let new_symbol = section
                        .vcd
                        .symbol_map
                        .get(&IdCode::from(symbol))
                        .unwrap_or_else(|| {
                            panic!(
                                "symbol not found: {:?}, {:?}",
                                &IdCode::from(symbol),
                                section.vcd.symbol_map
                            )
                        });

                    out_writer.write_all(name)?;
                    out_writer.write_all(new_symbol.as_bytes())?;
                    out_writer.write_all(b"\n")?;
                }
                [b'$', ..] => {
                    // println!("skipping {}", std::str::from_utf8(line).unwrap());
                }
                [] => {
                    // println!("empty line");
                }
                _ => {
                    let value = &line[0..1];
                    let symbol = &line[1..];
                    let new_symbol = section.vcd.symbol_map.get(&IdCode::from(symbol)).unwrap();

                    out_writer.write_all(value)?;
                    out_writer.write_all(new_symbol.as_bytes())?;
                    out_writer.write_all(b"\n")?;
                }
            }
        }

        // All lines in this section has been written
        PeekMut::pop(heap_entry);
    }

    Ok(())
}
