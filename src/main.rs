use std::net::{TcpListener, TcpStream};
use std::thread;
use std::io::{Read, Write};
use std::fs;
use std::path::{Path, PathBuf};
use std::env;
use percent_encoding::percent_decode_str;

fn main() {
    println!("Starting server...");

    let listener = TcpListener::bind("127.0.0.1:8080").expect("Creating a TCP listener failed");

    println!("Server started on {}.", listener.local_addr().unwrap());

    // Listen fpr incoming TCP/HTTP connections and handle each of them in a separate thread:
    for stream in listener.incoming()  {
        let stream = stream.expect("The iterator returned by incoming() will never return None");

        thread::spawn(|| {
            handle_connection(stream);
        });
    }
}

fn handle_connection(mut stream: TcpStream) {
    let addr = stream.peer_addr().unwrap(); // the socket address of the remote peer of this TCP connection

    // Read and parse the HTTP request:
    let mut http_request_buffer = [0; 1024];
    stream.read(&mut http_request_buffer).unwrap(); // "GET /[path] HTTP/1.1 [...]"
    let http_request = String::from_utf8_lossy(&http_request_buffer[..]);
    let get_path = http_request.split(' ').skip(1).next().unwrap_or("/");
    if !get_path.starts_with("/") {
        panic!("GET path does not start with a '/'!");
    }
    println!("{} requested {}", addr, get_path);

    // Turn the path from the URL/GET request into the path for the file system:
    //   1) Always use the parent directory of the binary as the root directory
    //   2) unescape the URL encoding ("%20" etc.)
    let binary_path: &String = &env::args().next().expect("Name of binary missing as 0th command line argument");
    let root_dir: &Path = Path::new(binary_path).parent().expect("Binary has no parent");
    let decoded_get_path: &str = &percent_decode_str(get_path).decode_utf8().unwrap();
    let fs_path_buffer: PathBuf = root_dir.join(&decoded_get_path[1..]); // The join function does not like when the path to adjoin starts with a '/'
    let fs_path: &Path = fs_path_buffer.as_path();

    // Create the HTTP response body/content:
    let mut content: Vec<u8> = match fs::read(fs_path) {
        Ok(data) => data, // The path specified a file which was successfully read, return the read data.
        Err(_) =>
            match fs::read_dir(fs_path) { // Returns an iterator over the entries within a directory.
                Ok(paths) => { // The path specified a directory which was successfully opened(/"read").
                    let mut folder_items: Vec<String> = paths
                        .map(|path| path.unwrap().path().strip_prefix(root_dir).unwrap().display().to_string()) // turn a path ("ReadDir") iterator into a String iterator
                        .collect(); // The only reason we collect into a Vector is so that we can sort the folder items alphabetically!
                    folder_items.sort(); // Display the folder items in alphabetical order.
                    folder_items.iter().map(|path| format!("<a href=\"{}\">{}</a><br>\r\n", path, path)) // turn the path Strings into HTML links
                        .fold(String::from(""), |str1, str2| str1 + &str2) // concatenate all the Strings of the iterator together into 1 single String
                        .into()
                },
                Err(error) => error.to_string().as_bytes().into() // The path specified neither a file nor a directory!
            }
    };

    // Now, create the complete HTTP response (with header containing the 'Content-Length'):
    let mut response: Vec<u8> = format!(
        "HTTP/1.1 200 OK\r\nContent-Length: {}\r\n\r\n",
        content.len()).as_bytes().into();
    response.append(&mut content);

    // Send the HTTP response to the client:
    stream.write(&response).unwrap();
    stream.flush().unwrap();

}
