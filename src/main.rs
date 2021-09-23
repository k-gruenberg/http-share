use percent_encoding::{percent_decode_str, utf8_percent_encode, NON_ALPHANUMERIC};
use std::env;
use std::fs;
use std::fs::File;
use std::io::{self, Read, Seek, SeekFrom, Write, Error, ErrorKind};
use std::net::{TcpListener, TcpStream};
use std::path::{Path, PathBuf};
use std::thread;

fn main() {
    println!(); // separator
    println!("Starting server...");

    let listener = match TcpListener::bind("0.0.0.0:8080") {
        Ok(listener) => listener,
        Err(err) => {
            println!("Error: Server could not be started as creating a TCP listener failed: {}", err);
            return;
        }
    };

    println!("Server started on {}.", listener.local_addr().unwrap());

    // Listen for incoming TCP/HTTP connections and handle each of them in a separate thread:
    for stream in listener.incoming() {
        let stream = stream.expect("The iterator returned by incoming() will never return None");

        thread::spawn(|| {
            handle_connection(stream).unwrap_or_else(|err_str| {println!("Error: {}", err_str)});
        });
    }
}

fn handle_connection(mut stream: TcpStream) -> std::io::Result<()> {
    // Read and parse the HTTP request:
    let http_request: HTTPRequest = match HTTPRequest::read_from_tcp_stream(&mut stream) {
        Ok(http_request) => http_request,
        Err(_err) => {
            HTTPResponse::new_500_server_error("Could not read HTTP request").send_to_tcp_stream(&mut stream)?;
            return Err(Error::new(ErrorKind::Other, format!("TCP stream from {:?} could not be read!", stream.peer_addr())));
        }
    };
    let get_path: &str = http_request.get_get_path();

    // Sanity check the requested GET path for security reasons:
    if !get_path.starts_with('/') {
        HTTPResponse::new_500_server_error("GET path does not start with a '/'!").send_to_tcp_stream(&mut stream)?;
        return Err(Error::new(ErrorKind::Other, format!("{:?} requested {} which does not start with a '/'!", stream.peer_addr(), get_path)));
    }

    // Log the HTTP request to console:
    println!("{} requested {}", stream.peer_addr().map_or("???".to_string(), |addr| addr.to_string()), get_path);

    // See if the requested URL contains a question mark ('?') and therefore a query string:
    let query_string: Option<&str> = if get_path.contains('?') {
        Some(get_path.split('?').nth(1).unwrap()) // unwrapping here is safe because we checked that it contains a '?'
    } else {
        None
    };
    // Now remove the query string from the GET path, if there is one
    let get_path: &str = get_path.split('?').nth(0).unwrap();

    // Turn the path from the URL/GET request into the path for the file system:
    //   1) Always use the parent directory of the binary as the root directory
    //   2) unescape the URL encoding ("%20" etc.)
    let binary_path: &String = &env::args().next().expect("Name of binary missing as 0th command line argument");
    let root_dir: &Path = Path::new(binary_path).parent().expect("Binary has no parent");
    let decoded_get_path: &str = &percent_decode_str(get_path).decode_utf8().unwrap();
    let fs_path_buffer: PathBuf = root_dir.join(&decoded_get_path[1..]); // The join function does not like when the path to adjoin starts with a '/'
    let fs_path: &Path = fs_path_buffer.as_path();

    // Create the HTTP response body/content:
    let path_metadata = match fs::metadata(fs_path) {
        Ok(metadata) => metadata,
        Err(_) => {
            HTTPResponse::new_404_not_found(fs_path.strip_prefix(root_dir).unwrap().to_string_lossy()).send_to_tcp_stream(&mut stream)?;
            // The '.strip_prefix' is important for not leaking the folder structure of the server to the web user!
            return Err(Error::new(ErrorKind::Other, format!("Could not find file {}", fs_path.display())));
        }
    };
    if path_metadata.is_dir() {
        if let Err(err) = dir_response(fs_path, root_dir, &mut stream, query_string) {
            HTTPResponse::new_500_server_error(err.to_string());
            return Err(Error::new(ErrorKind::Other, format!("Directory Response error: {}", err)));
        }
    } else {
        if let Err(err) = file_response(&http_request, fs_path, &mut stream) {
            HTTPResponse::new_500_server_error(err.to_string());
            return Err(Error::new(ErrorKind::Other, format!("File Response error: {}", err)));
        }
    };
    Ok(())
}

/// Responds to `stream` with the file contents queried by `filepath`.
fn file_response(http_request: &HTTPRequest, filepath: &Path, stream: &mut TcpStream) -> io::Result<()> {
    // Because of iOS we have to differentiate between 2 cases, a normal "full response" and a "range response" (for videos):
    if http_request.contains_range_header() {
        // iOS always requests ranges of video files and expects an according response!:
        // Parse the requested range from the request, so we can create the response for the iOS device:
        HTTPResponse::write_206_partial_file_to_stream(filepath, http_request.get_requested_range(), stream)?;
    } else {
        // The "normal" (either non-video or non-iOS) case, i.e. just return the entire content directly:
        HTTPResponse::write_200_ok_file_to_stream(filepath, stream)?;
    }
    Ok(())
}

/// Responds to `stream` with a list of all entries in `dir_path`.
/// The `root_dir` is given to know which prefix to strip from the file paths.
/// The optional `query_string` (what comes after the '?' in the URL) is given because it might
/// contain information on how to display the contents of the directory.
fn dir_response(dir_path: &Path, root_dir: &Path, stream: &mut TcpStream, query_string: Option<&str>) -> io::Result<()> {
    let mut folder_items: Vec<String> = fs::read_dir(dir_path)?
        .map(|path| { path.unwrap().path().strip_prefix(root_dir).unwrap().display().to_string() }) // turn a path ("ReadDir") iterator into a String iterator
        .collect(); // The only reason we collect into a Vector is so that we can sort the folder items alphabetically!
    let html_body: String = if !folder_items.is_empty() {
        folder_items.sort(); // Display the folder items in alphabetical order.
        format_body(folder_items, query_string, dir_path.strip_prefix(root_dir).unwrap().display().to_string())
    } else {
        "This folder is empty.".to_string() // Tell the user when a folder is empty instead of just giving him an empty page.
    };
    let http_response = HTTPResponse::new_200_ok(
        &mut format!(
            "<!DOCTYPE html><html><head><meta charset=\"utf-8\"/></head><body>\r\n{}</body></html>\r\n", // important because of the UTF-8!!
            html_body
        ).into()
    );
    http_response.send_to_tcp_stream(stream)
}

/// A helper function for `dir_response`.
/// Takes a Vec of the relative file paths in a folder as Strings (`folder_items`) and
/// returns the HTML body. The layout may differ depending on the `query_string` (the stuff that comes
/// after the '?' in the URL) given by the user.
/// The path of the current directory is given in `dir_path` as a String to let the user know where
/// he currently is.
fn format_body(folder_items: Vec<String>, query_string: Option<&str>, dir_path: String) -> String {
    let folder_items = folder_items.iter()
        .map(|path| { format_path(path, query_string) }); // turn the path Strings into HTML links, possibly within a <td>-tag

    let lower_body = match query_string {
        // Table View:
        Some("view=table") => format!(
            "<table style=\"table-layout:fixed;width:100%;\">\r\n{}</table>\r\n",
            folder_items
                .enumerate()
                .map(|(i, hyperlink)| {
                    match i % 3 {
                        0 => format!("<tr>\r\n{}", &hyperlink),
                        1 => hyperlink,
                        _ => format!("{}</tr>\r\n", &hyperlink)
                    }
                })
                .fold(String::from(""), |str1, str2| str1 + &str2)
        ),
        // Default = List View:
        _ => folder_items.fold(String::from(""), |str1, str2| str1 + &str2) // concatenate all the Strings of the iterator together into 1 single String
    };

    // At last, add the "header" (including links/buttons that let the user change the layout):
    return format!( // The leading slash ('/') of the path is added manually, cf. `format_path`.
        "/{}<br>\r\n\
         <a href=\"javascript:window.location.search='view=list';\">List View</a>  |  \r\n\
         <a href=\"javascript:window.location.search='view=table';\">Table View</a><br>\r\n\
         <hr><br>\r\n\
         {}",
        dir_path, lower_body
    );
}

/// A helper function for `format_body`.
/// Formats just the <a>-hyperlinks depending on the layout specified in the `query_string`.
fn format_path(path: &String, query_string: Option<&str>) -> String {
    // <a href="hyperlink">display_name</a>
    let hyperlink = utf8_percent_encode(path, NON_ALPHANUMERIC).to_string();
    let display_name = path.split('/').last().unwrap(); // only display the file name to the user

    match query_string {
        // Table View:
        Some("view=table") => format!("<td style=\"border: 1px solid black;\"><a href=\"/{}\"><img src=\"/{}\" alt=\"{}\" width=\"100%\"></a></td>\r\n", hyperlink, hyperlink, display_name),
        // Default = List View:
        _ => format!("<a href=\"/{}\">{}</a><br>\r\n", hyperlink, display_name) // The "/" is important!
    }
}

/// Takes a file system `Path` and returns the (HTML) content:
///   Case A) `fs_path` specifies a file: return the content of that file
///   Case B) `fs_path` specifies a directory: return a list of hyperlinks ('<a href>'s) to all the files in that dir
///   Case C) `fs_path` specifies neither a file nor a directory: return the error string
/// The `root_dir` argument is needed for Case B) to know how much of the path prefix to strip.
#[allow(dead_code)] // Old code, now see 'dir_response'
fn fs_path_to_content(fs_path: &Path, root_dir: &Path) -> Vec<u8> {
    match fs::read(fs_path) {
        Ok(data) => data, // The path specified a file which was successfully read, return the read data.
        Err(_) => match fs::read_dir(fs_path) {
            // Returns an iterator over the entries within a directory.
            Ok(paths) => {
                // The path specified a directory which was successfully opened(/"read").
                let mut folder_items: Vec<String> = paths.map(|path| {
                        path.unwrap().path().strip_prefix(root_dir).unwrap().display().to_string()
                    }).collect(); // The only reason we collect into a Vector is so that we can sort the folder items alphabetically!
                if !folder_items.is_empty() {
                    folder_items.sort(); // Display the folder items in alphabetical order.
                    folder_items.iter()
                        .map(|path| { format!("<a href=\"/{}\">{}</a><br>\r\n", path, path.split('/').last().unwrap()) }) // turn the path Strings into HTML links; The "/" is important!
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
        stream.read(&mut request_buffer)?; // "GET /[path] HTTP/1.1 [...]"
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
        let range = self.http_request.split("\r\n") // All request headers as separate lines
            .find(|s| s.starts_with("Range: bytes=")) // Take only the (correctly formatted) "Range" header
            .unwrap() // This is (essentially) safe because we checked that the string contains "Range: bytes=" above.
            .strip_prefix("Range: bytes=")
            .unwrap(); // This is safe because of the 'starts_with' check above. Now, `range` is string of the form "0-1".
        let mut start_and_end_index = range.split('-');
        let start_index = start_and_end_index.next().unwrap(); // (Unwrapping here should always work as `split` always returns at least 1 item.)
        let end_index = start_and_end_index.next().expect("range in 'Range' header is not of the form x-y");
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
        let mut http_response: Vec<u8> = format!("HTTP/1.1 200 OK\r\nContent-Length: {}\r\n\r\n", content.len()).as_bytes().into();
        http_response.append(content);
        Self { http_response }
    }

    /// Create a new 500 Internal Server Error response with the given `error_message`.
    fn new_500_server_error<T: AsRef<str>>(error_message: T) -> Self {
        let error_message = format!("Internal Server Error occurred: {}", error_message.as_ref());
        let http_response: Vec<u8> = format!("HTTP/1.1 500 Internal Server Error\r\nContent-Length: {}\r\n\r\n{}", error_message.len(), error_message).as_bytes().to_vec();
        Self { http_response }
    }

    /// Create a new 404 Not Found Error response.
    fn new_404_not_found<T: AsRef<str>>(filename: T) -> Self {
        let message = format!("Error: Could not find file {}", filename.as_ref());
        let http_response: Vec<u8> = format!("HTTP/1.1 404 Not Found\r\nContent-Length: {}\r\n\r\n{}", message.len(), message).as_bytes().to_vec();
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
        stream.write(format!("HTTP/1.1 200 OK\r\nContent-Length: {}\r\n\r\n", file_size).as_bytes())?;
        // Write file contents to stream
        io::copy(&mut file, stream)?;
        stream.flush()?;
        Ok(())
    }

    /// Create a new 206 Partial Content HTTP response.
    #[allow(dead_code)] // Only 'write_206_partial_file_to_stream' is actually used in this project, i.e. the more memory-efficient version for sending files.
    fn new_206_partial_content(content: &[u8], start_index: &str, end_index: &str) -> Self {
        // cf. https://stackoverflow.com/questions/23071164/grails-ios-specific-returning-video-mp4-file-gives-broken-pipe-exception-g
        let mut http_response: Vec<u8> = format!("HTTP/1.1 206 Partial Content\r\nAccept-Ranges: bytes\r\nContent-Range: bytes {}-{}/{}\r\n\r\n", start_index, end_index, content.len())
            .as_bytes().into();
        http_response.append(&mut content[start_index.parse().unwrap()..=end_index.parse().unwrap()].to_vec()); // Only respond with the requested bytes! "=" because end index in HTTP is inclusive (I think)
        return Self { http_response };
    }

    /// Directly writes the file contents of `filepath` to `stream` in range of bytes from `range`.
    fn write_206_partial_file_to_stream(filepath: &Path, range: (u64, u64), stream: &mut TcpStream) -> io::Result<()> {
        // Try to open the file before writing `206 Partial Content`, so that the HTTP status code can still be
        // changed in case of an error.
        let mut file = File::open(filepath)?;
        // Place read pointer at given start byte
        file.seek(SeekFrom::Start(range.0))?;
        // Only read bytes in given range from file
        let mut partial_file = file.take(range.1 - range.0 + 1); // +1 because end index in HTTP is inclusive!
        // Write http response header
        let file_size: u64 = File::open(filepath)?.metadata()?.len();
        stream.write(format!("HTTP/1.1 206 Partial Content\r\nAccept-Ranges: bytes\r\nContent-Range: bytes {}-{}/{}\r\n\r\n", range.0, range.1, file_size).as_bytes())?;
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
