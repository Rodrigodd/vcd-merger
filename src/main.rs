use fxhash::FxHashMap as HashMap;
use memmap2::Mmap;
use std::cmp::Reverse;
use std::collections::binary_heap::PeekMut;
use std::io::{BufRead, BufWriter, Write};
use std::sync::Mutex;

// this can only represent 94^4 = 78_074_896 symbols.
#[derive(Clone, Copy, Hash, PartialEq, Eq, Debug)]
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

struct Vcd {
    /// Map from old symbol to new symbol.
    symbol_map: HashMap<IdCode, IdCode>,
    /// All scope and var declarations.
    declarations: Vec<String>,
    file: Mmap,
    end_of_definitions: usize,
}

#[derive(Default)]
struct Header {
    date: Option<String>,
    version: Option<String>,
    timescale: Option<String>,
}

fn main() {
    let args = std::env::args().collect::<Vec<String>>();

    if args.len() < 3 {
        println!("usage: vcd-merger <input.vcd> [<input.vcd> *] <output.vcd>");
        return;
    }

    let inputs = &args[1..args.len() - 1];
    let output = &args[args.len() - 1];

    let mut headers = Header::default();

    let vcds = parse_headers(inputs, &mut headers);

    let sections = find_sections(&vcds);

    println!("split in {} sections", sections.len());

    write_output(output, headers, &vcds, sections).unwrap();
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
                    if header.timescale.is_none() {
                        header.timescale = Some(scale);
                    }
                }
                "$scope" => {
                    let module = tokens.next().unwrap();
                    let name = tokens.next().unwrap();
                    let end = tokens.next().unwrap();

                    println!("{} {}", module, name);

                    assert_eq!(end, "$end");

                    declarations.push(format!("$scope {} {} $end\n", module, name));
                }
                "$var" => {
                    let ty = tokens.next().unwrap();
                    let width = tokens.next().unwrap();
                    let old_id = tokens.next().unwrap();
                    let name = take_to_end(&mut tokens);

                    println!("{} {} {} {}", ty, width, old_id, name);

                    let new_id = next_code();

                    symbol_map.insert(IdCode::from(old_id.as_bytes()), new_id);

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
                    println!("$upscope");
                    declarations.push("$upscope $end\n".to_string());
                }
                "$enddefinitions" => {
                    let end = tokens.next().unwrap();
                    assert_eq!(end, "$end");
                    println!("$enddefinitions $end\n");
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

        let vcd = Vcd {
            symbol_map,
            declarations,
            end_of_definitions: reader.position() as usize,
            file: reader.into_inner(),
        };
        vcds.push(vcd);
    }
    vcds
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

// Find sections of sorted signal changes. These will be merged sorted when written to the output
// file.
fn find_sections(vcds: &[Vcd]) -> Vec<Section> {
    let mut sections = Vec::new();

    for vcd in vcds {
        let lines = vcd.file[vcd.end_of_definitions..].split(|&b| b == b'\n');
        let mut curr_section = None;

        for line in lines {
            if let [b'#', ..] = line {
                let offset = line.as_ptr() as usize - vcd.file.as_ptr() as usize;
                let curr_line_value = parse_u64(&line[1..]).unwrap();

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
    }

    sections
}

fn write_output<'a>(
    output: &String,
    headers: Header,
    vcds: &'a [Vcd],
    mut sections: Vec<Section<'a>>,
) -> std::io::Result<()> {
    let out_file = std::fs::File::create(output).unwrap();
    let mut out_writer = BufWriter::with_capacity(0x1_0000, out_file); // 64KiB

    let total_len = sections.iter().map(|s| s.section.len() as u64).sum::<u64>();
    const TEMPLATE: &str =
        "{elapsed_precise} █{bar:60.cyan/blue}█ {bytes}/{total_bytes} {binary_bytes_per_sec} ({eta})";
    let bar = indicatif::ProgressBar::new(total_len).with_style(
        indicatif::ProgressStyle::default_bar()
            .template(TEMPLATE)
            .unwrap()
            .progress_chars("█▉▊▋▌▍▎▏  "),
    );

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

    'sections: while let Some(mut heap_entry) = heap.peek_mut() {
        let Reverse((_, index)) = *heap_entry;
        let section = &mut sections[index];
        let mut lines = section.section.split(|x| *x == b'\n');

        // skip the first line
        if let Some(line) = lines.next() {
            out_writer.write_all(line)?;
            out_writer.write_all(b"\n")?;
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
                bar.set_position(progress);
            }

            match &line {
                [b'#', ..] => {
                    let offset = line.as_ptr() as usize - section.section.as_ptr() as usize;
                    let value = parse_u64(&line[1..]).unwrap();
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

    bar.finish();

    Ok(())
}
