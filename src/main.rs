use fxhash::FxHashMap as HashMap;
use std::io::{BufRead, BufReader, BufWriter, Write};
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

    for (i, c) in code.0.iter_mut().enumerate() {
        // '~'
        if *c == 0x0 {
            // '!'
            *c = 0x21;
            break;
        }
        if *c < 0x7E {
            *c += 1;
            break;
        } else {
            // '!'
            *c = 0x21;
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

fn parse_headers(inputs: &[String], header: &mut Header) -> Vec<Vcd<impl BufRead>> {
    let mut vcds = Vec::new();
    for input in inputs {
        let file = std::fs::File::open(input).unwrap();
        let mut reader = BufReader::new(file);

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

                    declarations.push(format!("$scope {} {}", module, name));
                }
                "$var" => {
                    let ty = tokens.next().unwrap();
                    let width = tokens.next().unwrap();
                    let id = tokens.next().unwrap();
                    let name = take_to_end(&mut tokens);

                    println!("{} {} {} {}", ty, width, id, name);

                    let new_id = next_code();

                    symbol_map.insert(IdCode::from(id.as_bytes()), new_id);

                    declarations.push(format!("$var {} {} {} {}", ty, width, id, name));
                }
                "$upscope" => {
                    let end = tokens.next().unwrap();
                    assert_eq!(end, "$end");
                    println!("$upscope");
                    declarations.push("$upscope".to_string());
                }
                "$enddefinitions" => {
                    let end = tokens.next().unwrap();
                    assert_eq!(end, "$end");
                    println!("$enddefinitions");
                    declarations.push("$enddefinitions".to_string());
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
    vcds: Vec<Vcd<impl BufRead>>,
) -> std::io::Result<()> {
    let out_file = std::fs::File::create(output).unwrap();
    let mut out_writer = BufWriter::new(out_file);

    if let Some(date) = headers.date {
        out_writer.write_all(b"$date ")?;
        out_writer.write_all(date.as_bytes())?;
    }
    if let Some(version) = headers.version {
        out_writer.write_all(b"$version ")?;
        out_writer.write_all(version.as_bytes())?;
    }
    if let Some(timescale) = headers.timescale {
        out_writer.write_all(b"$timescale ")?;
        out_writer.write_all(timescale.as_bytes())?;
    }

    for vcd in vcds.iter() {
        for line in vcd.declarations.iter() {
            out_writer.write_all(line.as_bytes())?;
        }
    }

    writeln!(out_writer, "$dumpvars").unwrap();

    for mut vcd in vcds {
        let mut line_buf = Vec::new();

        while vcd.reader.read_until(b'\n', &mut line_buf)? > 0 {
            let line = &line_buf[..line_buf.len() - 1];

            match &line {
                [b'#', ..] => {
                    out_writer.write_all(&line_buf)?;
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
                _ => {
                    let value = &line[0..1];
                    let symbol = &line[1..];
                    let new_symbol = vcd.symbol_map.get(&IdCode::from(symbol)).unwrap();
                    out_writer.write_all(value)?;
                    out_writer.write_all(new_symbol.as_bytes())?;
                    out_writer.write_all(b"\n")?;
                }
            }

            line_buf.clear()
        }
    }

    Ok(())
}
