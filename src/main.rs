use fxhash::FxHashMap as HashMap;
use std::io::{BufRead, BufWriter, Cursor, Write};
use std::sync::Mutex;

// this can only represent 93^4 = 74_805_201 symbols.
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

struct Vcd<B: BufRead> {
    /// Map from old symbol to new symbol.
    symbol_map: HashMap<IdCode, IdCode>,
    /// All scope and var declarations.
    declarations: Vec<String>,
    reader: B,
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

    write_output(output, headers, vcds).unwrap();
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

fn parse_headers(inputs: &[String], header: &mut Header) -> Vec<Vcd<Cursor<memmap2::Mmap>>> {
    let mut vcds = Vec::new();
    for input in inputs {
        let file = std::fs::File::open(input).unwrap();
        // let mut reader = BufReader::with_capacity(0x1_0000, file);
        let memmap = unsafe { memmap2::MmapOptions::new().map(&file).unwrap() };
        let mut reader = std::io::Cursor::new(memmap);

        let mut lines = (&mut reader).lines().map_while(Result::ok);

        let mut tokens = lines.by_ref().flat_map(|line| {
            line.split_whitespace()
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
            reader,
        };
        vcds.push(vcd);
    }
    vcds
}

fn write_output(
    output: &String,
    headers: Header,
    mut vcds: Vec<Vcd<Cursor<memmap2::Mmap>>>,
) -> std::io::Result<()> {
    let out_file = std::fs::File::create(output).unwrap();
    let mut out_writer = BufWriter::with_capacity(0x1_0000, out_file); // 64KiB

    if let Some(date) = headers.date {
        out_writer.write_all(b"$date ")?;
        out_writer.write_all(date.as_bytes())?;
        out_writer.write_all(b" $end\n")?;
    }
    if let Some(version) = headers.version {
        out_writer.write_all(b"$version ")?;
        out_writer.write_all(version.as_bytes())?;
        out_writer.write_all(b" $end\n")?;
    }
    if let Some(timescale) = headers.timescale {
        out_writer.write_all(b"$timescale ")?;
        out_writer.write_all(timescale.as_bytes())?;
        out_writer.write_all(b" $end\n")?;
    }

    for vcd in vcds.iter_mut() {
        for line in vcd.declarations.iter() {
            out_writer.write_all(line.as_bytes())?;
        }
        vcd.declarations = Vec::new();
    }

    out_writer.write_all(b"$enddefinitions $end\n")?;

    writeln!(out_writer, "$dumpvars").unwrap();

    for vcd in vcds {
        let memmap = vcd.reader.into_inner();
        for line in memmap.split(|x| *x == b'\n') {
            match &line {
                [b'#', ..] => {
                    out_writer.write_all(line)?;
                    out_writer.write_all(b"\n")?;
                }
                [b'b', ..] | [b'r', ..] => {
                    let pos = line.iter().position(|c| *c == b' ').unwrap();
                    let (name, symbol) = line.split_at(pos + 1);
                    let new_symbol =
                        vcd.symbol_map
                            .get(&IdCode::from(symbol))
                            .unwrap_or_else(|| {
                                panic!(
                                    "symbol not found: {:?}, {:?}",
                                    &IdCode::from(symbol),
                                    vcd.symbol_map
                                )
                            });
                    out_writer.write_all(name)?;
                    out_writer.write_all(new_symbol.as_bytes())?;
                    out_writer.write_all(b"\n")?;
                }
                [b'$', ..] => {
                    println!("skipping {}", std::str::from_utf8(line).unwrap());
                }
                [] => {
                    println!("empty line");
                }
                _ => {
                    let value = &line[0..1];
                    let symbol = &line[1..];
                    let new_symbol = vcd.symbol_map.get(&IdCode::from(symbol)).unwrap();
                    out_writer.write_all(value)?;
                    out_writer.write_all(new_symbol.as_bytes())?;
                    out_writer.write_all(b"\n")?;
                }
            }
        }
    }

    Ok(())
}
