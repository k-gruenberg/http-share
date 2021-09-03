use std::net::{TcpListener, TcpStream};
use std::thread;
use std::io::{Read, Write};
use std::fs;
use std::path::{Path, PathBuf};
use std::env;
use percent_encoding::percent_decode_str;

fn main() {
    println!("Starting server...");

    let listener = TcpListener::bind("0.0.0.0:8080").expect("Creating a TCP listener failed");

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

    // Now, create the complete HTTP response with headers:
    let mut response: Vec<u8>;
    // Because of iOS we have to differentiate between 2 cases, a normal "full response" and a "range response" (for videos):
    if http_request.contains("Range: bytes=") { // iOS always requests ranges of video files and expects an according response!:
        // cf. https://stackoverflow.com/questions/23071164/grails-ios-specific-returning-video-mp4-file-gives-broken-pipe-exception-g
        let range = http_request
            .split("\r\n") // All request headers as separate lines
            .filter(|s| s.starts_with("Range: bytes=")) // Take only the (correctly formatted) "Range" header
            .next()
            .unwrap() // This is (essentially) safe because we checked that the string contains "Range: bytes=" above.
            .strip_prefix("Range: bytes=")
            .unwrap(); // This is safe because of the 'starts_with' check above.
        // Now, `range` is string of the form "0-1".
        let mut start_and_end_index = range.split('-');
        let start_index = start_and_end_index.next().unwrap(); // (Unwrapping here should always work as `split` always returns at least 1 item.)
        let end_index = start_and_end_index.next().expect("range in 'Range' header is not of the form x-y");

        // Now, we can finally create the response for the iOS device:
        response = format!(
            "HTTP/1.1 206 Partial Content\r\nAccept-Ranges: bytes\r\nContent-Range: bytes {}-{}/{}\r\n\r\n",
            start_index, end_index, content.len()).as_bytes().into();
        response.append(&mut content[start_index.parse().unwrap()..=end_index.parse().unwrap()].to_vec()); // Only respond with the requested bytes! "=" because end index in HTTP is inclusive (I think)
    } else { // The "normal" (either non-video or non-iOS) case, i.e. just return the entire content directly:
        response = format!(
            "HTTP/1.1 200 OK\r\nContent-Length: {}\r\n\r\n",
            content.len()).as_bytes().into();
        response.append(&mut content);
    }

    // Send the HTTP response to the client:
    stream.write(&response).unwrap_or_else(|err_str| {println!("Response Error ({}): {}", addr, err_str); 0});
    stream.flush().unwrap_or_else(|_| println!("Error when flushing response stream!"));

}
