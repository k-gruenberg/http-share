use percent_encoding::percent_decode_str;
use std::env;
use std::fs;
use std::fs::File;
use std::io::{self, Read, Seek, SeekFrom, Write};
use std::net::{TcpListener, TcpStream};
use std::path::{Path, PathBuf};
use std::thread;

fn main() {
    println!("Starting server...");

    let listener = TcpListener::bind("0.0.0.0:8080").expect("Creating a TCP listener failed");

    println!("Server started on {}.", listener.local_addr().unwrap());

    // Listen fpr incoming TCP/HTTP connections and handle each of them in a separate thread:
    for stream in listener.incoming() {
        let stream = stream.expect("The iterator returned by incoming() will never return None");

        thread::spawn(|| {
            handle_connection(stream);
        });
    }
}

fn handle_connection(mut stream: TcpStream) {
    // Read and parse the HTTP request:
    let http_request: HTTPRequest = match HTTPRequest::read_from_tcp_stream(&mut stream) {
        Ok(http_request) => http_request,
        Err(_err) => {
            HTTPResponse::new_500_server_error("Could not read HTTP request")
                .send_to_tcp_stream(&mut stream);
            return;
        }
    };
    let get_path: &str = http_request.get_get_path();

    // Sanity check the requested GET path for security reasons:
    if !get_path.starts_with('/') {
        HTTPResponse::new_500_server_error("GET path does not start with a '/'!")
            .send_to_tcp_stream(&mut stream);
        return;
    }

    // Log the HTTP request to console:
    println!("{:?} requested {}", stream.peer_addr(), get_path);

    // Turn the path from the URL/GET request into the path for the file system:
    //   1) Always use the parent directory of the binary as the root directory
    //   2) unescape the URL encoding ("%20" etc.)
    let binary_path: &String = &env::args()
        .next()
        .expect("Name of binary missing as 0th command line argument");
    let root_dir: &Path = Path::new(binary_path)
        .parent()
        .expect("Binary has no parent");
    let decoded_get_path: &str = &percent_decode_str(get_path).decode_utf8().unwrap();
    let fs_path_buffer: PathBuf = root_dir.join(&decoded_get_path[1..]); // The join function does not like when the path to adjoin starts with a '/'
    let fs_path: &Path = fs_path_buffer.as_path();

    // Create the HTTP response body/content:
    let path_metadata = match fs::metadata(fs_path) {
        Ok(metadata) => metadata,
        Err(_) => {
            HTTPResponse::new_400_not_found(fs_path.to_string_lossy())
                .send_to_tcp_stream(&mut stream);
            return;
        }
    };
    if path_metadata.is_dir() {
        if let Err(err) = dir_response(fs_path, root_dir, &mut stream) {
            HTTPResponse::new_500_server_error(err.to_string());
            return;
        }
    } else {
        if let Err(err) = file_response(&http_request, fs_path, &mut stream) {
            HTTPResponse::new_500_server_error(err.to_string());
            return;
        }
    }
}

/// Responds to `stream` with the file contents queried by `filepath`.
fn file_response(
    http_request: &HTTPRequest,
    filepath: &Path,
    stream: &mut TcpStream,
) -> io::Result<()> {
    // Because of iOS we have to differentiate between 2 cases, a normal "full response" and a "range response" (for videos):
    if http_request.contains_range_header() {
        // iOS always requests ranges of video files and expects an according response!:
        // Now that we know the requested range, we can create the response for the iOS device:
        // Only respond with the requested bytes! "=" because end index in HTTP is inclusive (I think):
        HTTPResponse::write_206_partial_file_to_stream(
            filepath,
            http_request.get_requested_range(),
            stream,
        )?;
    } else {
        // The "normal" (either non-video or non-iOS) case, i.e. just return the entire content directly:
        HTTPResponse::write_200_ok_file_to_stream(filepath, stream)?;
    }
    Ok(())
}

/// Responds to `stream` with a list of all entries in `dirpath`.
fn dir_response(dirpath: &Path, root_dir: &Path, stream: &mut TcpStream) -> io::Result<()> {
    let mut folder_items: Vec<String> = fs::read_dir(dirpath)?
        .map(|path| {
            path.unwrap()
                .path()
                .strip_prefix(root_dir)
                .unwrap()
                .display()
                .to_string()
        }) // turn a path ("ReadDir") iterator into a String iterator
        .collect(); // The only reason we collect into a Vector is so that we can sort the folder items alphabetically!
    let mut response: Vec<u8> = if !folder_items.is_empty() {
        folder_items.sort(); // Display the folder items in alphabetical order.
        folder_items
            .iter()
            .map(|path| {
                format!(
                    "<a href=\"/{}\">{}</a><br>\r\n",
                    path,
                    path.split('/').last().unwrap()
                )
            }) // turn the path Strings into HTML links; The "/" is important!
            .fold(String::from(""), |str1, str2| str1 + &str2) // concatenate all the Strings of the iterator together into 1 single String
            .into()
    } else {
        "This folder is empty.".into() // Tell the user when a folder is empty instead of just giving him an
                                       // empty page.
    };
    let http_response = HTTPResponse::new_200_ok(&mut response);
    http_response.send_to_tcp_stream(stream)
}

/// Takes a file system `Path` and returns the (HTML) content:
///   Case A) `fs_path` specifies a file: return the content of that file
///   Case B) `fs_path` specifies a directory: return a list of hyperlinks ('<a href>'s) to all the files in that dir
///   Case C) `fs_path` specifies neither a file nor a directory: return the error string
/// The `root_dir` argument is needed for Case B) to know how much of the path prefix to strip.
fn fs_path_to_content(fs_path: &Path, root_dir: &Path) -> Vec<u8> {
    match fs::read(fs_path) {
        Ok(data) => data, // The path specified a file which was successfully read, return the read data.
        Err(_) => match fs::read_dir(fs_path) {
            // Returns an iterator over the entries within a directory.
            Ok(paths) => {
                // The path specified a directory which was successfully opened(/"read").
                let mut folder_items: Vec<String> = paths
                    .map(|path| {
                        path.unwrap()
                            .path()
                            .strip_prefix(root_dir)
                            .unwrap()
                            .display()
                            .to_string()
                    }) // turn a path ("ReadDir") iterator into a String iterator
                    .collect(); // The only reason we collect into a Vector is so that we can sort the folder items alphabetically!
                if !folder_items.is_empty() {
                    folder_items.sort(); // Display the folder items in alphabetical order.
                    folder_items
                        .iter()
                        .map(|path| {
                            format!(
                                "<a href=\"/{}\">{}</a><br>\r\n",
                                path,
                                path.split('/').last().unwrap()
                            )
                        }) // turn the path Strings into HTML links; The "/" is important!
                        .fold(String::from(""), |str1, str2| str1 + &str2) // concatenate all the Strings of the iterator together into 1 single String
                        .into()
                } else {
                    "This folder is empty.".into() // Tell the user when a folder is empty instead of just giving him an empty page.
                }
            }
            Err(error) => error.to_string().as_bytes().into(), // The path specified neither a file nor a directory!
        },
    }
}

/// A wrapper around a `String` representing an HTTP request.
struct HTTPRequest {
    http_request: String,
}

impl HTTPRequest {
    /// Create a new `HTTPRequest` by reading an HTTP request from a `TcpStream`.
    fn read_from_tcp_stream(stream: &mut TcpStream) -> io::Result<Self> {
        let mut request_buffer = [0u8; 1024];
        stream.read_exact(&mut request_buffer)?; // "GET /[path] HTTP/1.1 [...]"
        return Ok(Self {
            http_request: String::from_utf8_lossy(&request_buffer).to_string(),
        });
    }

    /// Get the requested path of this GET request.
    fn get_get_path(&self) -> &str {
        // An HTTP GET request starts like so: "GET /[path] HTTP/1.1 [...]".
        // Split that String by ' ', skip the "GET" and return the path:
        self.http_request.split(' ').nth(1).unwrap_or("/")
    }

    /// Whether this HTTP request contains a 'Range' header.
    fn contains_range_header(&self) -> bool {
        self.http_request.contains("Range: bytes=")
    }

    /// This function will panic when this HTTP request contains no (or an invalid) 'Range' header.
    /// Check using the `contains_range_header` function beforehand!
    fn get_requested_range(&self) -> (u64, u64) {
        // cf. https://stackoverflow.com/questions/23071164/grails-ios-specific-returning-video-mp4-file-gives-broken-pipe-exception-g
        let range = self
            .http_request
            .split("\r\n") // All request headers as separate lines
            .find(|s| s.starts_with("Range: bytes=")) // Take only the (correctly formatted) "Range" header
            .unwrap() // This is (essentially) safe because we checked that the string contains "Range: bytes=" above.
            .strip_prefix("Range: bytes=")
            .unwrap(); // This is safe because of the 'starts_with' check above.
                       // Now, `range` is string of the form "0-1".
        let mut start_and_end_index = range.split('-');
        let start_index = start_and_end_index.next().unwrap(); // (Unwrapping here should always work as `split` always returns at least 1 item.)
        let end_index = start_and_end_index
            .next()
            .expect("range in 'Range' header is not of the form x-y");
        return (start_index.parse().unwrap(), end_index.parse().unwrap());
    }
}

impl From<String> for HTTPRequest {
    fn from(http_request: String) -> Self {
        Self { http_request }
    }
}
impl From<HTTPRequest> for String {
    fn from(http_request: HTTPRequest) -> Self {
        http_request.http_request
    }
}

/// A wrapper around a `Vec<u8>` representing an HTTP response.
struct HTTPResponse {
    http_response: Vec<u8>,
}

impl HTTPResponse {
    /// Create a new 200 OK HTTP response.
    fn new_200_ok(content: &mut Vec<u8>) -> Self {
        let mut http_response: Vec<u8> = format!(
            "HTTP/1.1 200 OK\r\nContent-Length: {}\r\n\r\n",
            content.len()
        )
        .as_bytes()
        .into();
        http_response.append(content);
        Self { http_response }
    }

    /// Create a new 500 Internal Server Error response with the given `error_message`.
    fn new_500_server_error<T: AsRef<str>>(error_message: T) -> Self {
        let http_response: Vec<u8> = format!(
            "HTTP/1.1 500 Internal Server Error\r\n\
            Content-Length: {}\r\n\r\n\
            <h1>Internal Server Error occurred</h1>{}",
            error_message.as_ref().len(),
            error_message.as_ref()
        )
        .as_bytes()
        .to_vec();
        Self { http_response }
    }

    /// Create a new 500 Internal Server Error response with the given `error_message`.
    fn new_400_not_found<T: AsRef<str>>(filename: T) -> Self {
        let http_response: Vec<u8> = format!(
            "HTTP/1.1 404 Not Found\r\n\
            Content-Length: {}\r\n\r\n\
            Could not find file {}",
            filename.as_ref().len(),
            filename.as_ref()
        )
        .as_bytes()
        .to_vec();
        Self { http_response }
    }

    /// Directly writes the file contents of `filepath` to `stream`.
    ///
    /// By using file metadata to query the size of the file from the operating system, reading the entire
    /// file into memory only to get its size is avoided, which can save a lot of memory for large files.
    fn write_200_ok_file_to_stream(filepath: &Path, stream: &mut TcpStream) -> io::Result<()> {
        // Try to open the file before writing `200 OK`, so that the HTTP status code can still be changed in case of an
        // error.
        let mut file = File::open(filepath)?;
        let file_size = file.metadata()?.len();
        // Write http response header
        stream.write(b"HTTP/1.1 200 OK\r\nContent-Length: ");
        stream.write(file_size.to_string().as_bytes());
        stream.write(b"\r\n\r\n");
        // Write file contents to stream
        io::copy(&mut file, stream);
        stream.flush()?;
        Ok(())
    }

    /// Create a new 206 Partial Content HTTP response.
    fn new_206_partial_content(content: &[u8], start_index: &str, end_index: &str) -> Self {
        // cf. https://stackoverflow.com/questions/23071164/grails-ios-specific-returning-video-mp4-file-gives-broken-pipe-exception-g
        let mut http_response: Vec<u8> = format!(
            "HTTP/1.1 206 Partial Content\r\nAccept-Ranges: bytes\r\nContent-Range: bytes {}-{}/{}\r\n\r\n",
            start_index, end_index, content.len()).as_bytes().into();
        http_response.append(
            &mut content[start_index.parse().unwrap()..=end_index.parse().unwrap()].to_vec(),
        ); // Only respond with the requested bytes! "=" because end index in HTTP is inclusive (I think)
        return Self { http_response };
    }

    /// Directly writes the file contents of `filepath` to `stream` in range of bytes from `range`.
    fn write_206_partial_file_to_stream(
        filepath: &Path,
        range: (u64, u64),
        stream: &mut TcpStream,
    ) -> io::Result<()> {
        // Try to open the file before writing `206 Partial Content`, so that the HTTP status code can still be
        // changed in case of an error.
        let mut file = File::open(filepath)?;
        // Place read pointer at given start byte
        file.seek(SeekFrom::Start(range.0))?;
        // Only read bytes in given range from file
        let mut partial_file = file.take(range.1 - range.0);
        // Write http response header
        stream.write(
            b"HTTP/1.1 206 Partial Content\r\nAccept-Ranges: bytes\r\nContent-Range: bytes ",
        );
        stream.write(range.0.to_string().as_bytes());
        stream.write(b"-");
        stream.write(range.1.to_string().as_bytes());
        stream.write(b"\r\n\r\n");
        // Write file contents to stream
        io::copy(&mut partial_file, stream)?;
        stream.flush()?;
        Ok(())
    }

    /// Send the created HTTP response to a stream. An IO error may occur, e.g. a "Broken pipe".
    fn send_to_tcp_stream(&self, stream: &mut TcpStream) -> std::io::Result<()> {
        // Send the HTTP response to the client:
        stream.write_all(&self.http_response)?;
        stream.flush()?;
        Ok(())
    }
}
