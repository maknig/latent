use ort::session::Session;
fn main() {
    for p in std::env::args().skip(1) {
        let s = Session::builder().unwrap().commit_from_file(&p).unwrap();
        println!("{p}");
        for i in s.inputs.iter() { println!("  in  {} {:?}", i.name, i.input_type); }
        for o in s.outputs.iter() { println!("  out {} {:?}", o.name, o.output_type); }
    }
}
