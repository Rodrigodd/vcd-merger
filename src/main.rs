use std::collections::HashMap;
use std::io::{BufRead, BufReader, BufWriter, Write};
use std::sync::Mutex;

#[derive(Default)]
struct Vcd<T: Iterator<Item = String>> {
    /// Map from old symbol to new symbol.
    symbol_map: HashMap<String, String>,
    /// All scope and var declarations.
    declarations: Vec<String>,
    lines: T,
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

    let out_file = std::fs::File::create(output).unwrap();
    let mut out_writer = BufWriter::new(out_file);

    if let Some(date) = headers.date {
        writeln!(out_writer, "$date {}", date).unwrap();
    }
    if let Some(version) = headers.version {
        writeln!(out_writer, "$version {}", version).unwrap();
    }
    if let Some(timescale) = headers.timescale {
        writeln!(out_writer, "$timescale {}", timescale).unwrap();
    }

    for vcd in vcds.iter() {
        for line in vcd.declarations.iter() {
            writeln!(out_writer, "{}", line).unwrap();
        }
    }

    writeln!(out_writer, "$dumpvars").unwrap();

    for vcd in vcds {
        for line in vcd.lines {
            if line.starts_with('#') {
                writeln!(out_writer, "{}", line).unwrap();
            } else if line.starts_with('b') || line.starts_with('r') {
                let (name, symbol) = line.split_once(' ').unwrap();
                let new_symbol = vcd.symbol_map.get(symbol).unwrap_or_else(|| {
                    // println!("{:?}", vcd.symbol_map);
                    panic!("symbol not found: {:?}", symbol);
                });
                writeln!(out_writer, "{} {}", name, new_symbol).unwrap();
            } else if line.starts_with('$') {
                println!("skipping {}", line);
            } else {
                let value = &line[0..1];
                let symbol = &line[1..];
                let new_symbol = vcd.symbol_map.get(symbol).unwrap_or_else(|| {
                    // println!("{:?}", vcd.symbol_map);
                    panic!("symbol not found: {:?}", symbol);
                });
                writeln!(out_writer, "{}{}", value, new_symbol).unwrap();
            }
        }
    }
}

fn next_code() -> String {
    static CURR_CODE: Mutex<Vec<u8>> = Mutex::new(Vec::new()); // '!'
    let mut code = CURR_CODE.lock().unwrap();

    if code.is_empty() {
        code.push(0x21);
    }

    let next = String::from_utf8(code.clone()).unwrap();

    let len = code.len();
    for (i, c) in code.iter_mut().enumerate() {
        // '~'
        if *c < 0x7E {
            *c += 1;
            break;
        } else {
            // '!'
            *c = 0x21;
            if i == len - 1 {
                code.push(0x21);
                break;
            }
        }
    }

    next
}

fn parse_headers(inputs: &[String], header: &mut Header) -> Vec<Vcd<impl Iterator<Item = String>>> {
    let mut vcds = Vec::new();
    for input in inputs {
        let file = std::fs::File::open(input).unwrap();
        let reader = BufReader::new(file);

        let mut lines = reader.lines().map_while(|x| x.ok());

        let mut tokens = lines.by_ref().flat_map(|line| {
            line.split_whitespace()
                .map(String::from)
                .collect::<Vec<_>>()
        });

        let mut symbol_map = HashMap::new();

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

                    symbol_map.insert(id.to_string(), new_id);

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
            lines,
        };
        vcds.push(vcd);
    }
    vcds
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
