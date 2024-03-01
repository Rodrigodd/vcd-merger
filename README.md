# VCD Merger

A tool for merging multiple VCD (Value Change Dump) files together. This will
concatenate all signals from all the input files, side-by-side, merge-sorting
the timestamps, making it easier to view all files at the same time in a wave
visualizer, like GTKWave.

![2024-02-29_22h29m15s_screenshot](https://github.com/Rodrigodd/vcd-merger/assets/51273772/1e21c935-eca1-42e0-ba80-709f416410ad)

In the example two traces ("gameroy" and "dmg_cpu_b_gameboy") where combined
in a single file for easier visualization. The merged VCD were converted to
FST first (using GTKWave's `vcd2fst`), to speed up loading in GTKWave.

## Usage

```shell
vcd-merger input1.vcd input2.vcd ... output.vcd
```

## Limitations

- Does not validate the input file, will either panic or produce invalid output
  in that case.
- Only support files with a maximum of 78 million (94^4) variables.
- Don't preseve commands like `$dumpvar`, `$dumpon`, etc. Please, issue a new
  issue if this is actually an issue for you.

## Similar Projects

- [louiscaron/vcd_merge](https://github.com/louiscaron/vcd_merge): The only
  project I could find that does what I needed. But it was too slow for the 5.5
  GiB file I need to process, so I implemented my own.

## License

Licensed under either of

 * Apache License, Version 2.0, ([LICENSE-APACHE](LICENSE-APACHE) or
   http://www.apache.org/licenses/LICENSE-2.0)
 * MIT license ([LICENSE-MIT](LICENSE-MIT) or
   http://opensource.org/licenses/MIT)

at your option.
