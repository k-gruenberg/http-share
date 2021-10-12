use std::fs::File;
use std::io::{self, Read, Write, Seek, SeekFrom};
use std::net::TcpStream;
use std::path::Path;
use std::fmt::Display;

/// A wrapper around a `String` representing an HTTP request.
pub struct HTTPRequest {
    http_request: String,
}

impl HTTPRequest {
    /// Create a new `HTTPRequest` by reading an HTTP request from a `TcpStream`.
    pub fn read_from_tcp_stream(stream: &mut TcpStream) -> io::Result<Self> {
        let mut request_buffer = [0u8; 1024];
        stream.read(&mut request_buffer)?; // "GET /[path] HTTP/1.1 [...]"
        return Ok(Self {
            http_request: String::from_utf8_lossy(&request_buffer).to_string(),
        });
    }

    /// Get the requested path of this GET request.
    pub fn get_get_path(&self) -> &str {
        // An HTTP GET request starts like so: "GET /[path] HTTP/1.1 [...]".
        // Split that String by ' ', skip the "GET" and return the path:
        self.http_request.split(' ').nth(1).unwrap_or("/")
    }

    /// Whether this HTTP request contains a 'Range' header.
    pub fn contains_range_header(&self) -> bool {
        self.http_request.contains("Range: bytes=")
    }

    /// This function will panic when this HTTP request contains no (or an invalid) 'Range' header.
    /// Check using the `contains_range_header` function beforehand!
    ///
    /// For more information on the HTTP 'Range' header, see:
    /// https://developer.mozilla.org/en-US/docs/Web/HTTP/Headers/Range
    /// Currently only the following 2 formats are supported!:
    /// * Range: <unit>=<range-start>-
    /// * Range: <unit>=<range-start>-<range-end>
    pub fn get_requested_range(&self) -> (u64, Option<u64>) {
        // cf. https://stackoverflow.com/questions/23071164/grails-ios-specific-returning-video-mp4-file-gives-broken-pipe-exception-g
        let range = self.http_request.split("\r\n") // All request headers as separate lines
            .find(|s| s.starts_with("Range: bytes=")) // Take only the (correctly formatted) "Range" header
            .unwrap() // This is (essentially) safe because we checked that the string contains "Range: bytes=" above.
            .strip_prefix("Range: bytes=")
            .unwrap(); // This is safe because of the 'starts_with' check above. Now, `range` is string of the form "0-1" or "0-" (no <range-end>).
        let mut start_and_end_index = range.split('-');
        let start_index = start_and_end_index.next().unwrap(); // (Unwrapping here should always work as `split` always returns at least 1 item.)
        let end_index = start_and_end_index.next().expect("range in 'Range' header is not of the form x-y");
        return (start_index.parse().unwrap(), end_index.parse().ok());
    }

    /// Get the username and password the user provided as authorization (if he did).
    /// Reads the 'Authorization' header of this HTTP request, decodes it (Base64) and returns
    /// `Some((username, password))` or `None` when no (or an invalid) 'Authorization' header was
    /// provided.
    /// Only the 'Basic' authentication scheme is supported.
    pub fn get_authorization(&self) -> Option<(String, String)> {
        // https://de.wikipedia.org/wiki/HTTP-Authentifizierung
        //   Example: "Authorization: Basic d2lraTpwZWRpYQ=="
        //            where "d2lraTpwZWRpYQ==" is the Base64 encoding of "wiki:pedia"
        //            which stands for username "wiki" and password "pedia"
        let base64_encoded = self.http_request.split("\r\n") // All request headers as separate lines
            .find(|s| s.starts_with("Authorization: Basic "))? // Take only the (correctly formatted) "Authorization" header
            .strip_prefix("Authorization: Basic ")?;
        let base64_decoded = String::from_utf8(base64::decode(base64_encoded).ok()?).ok()?;
        let mut uname_and_pw = base64_decoded.split(":");
        return Some((uname_and_pw.next()?.to_string(), uname_and_pw.next()?.to_string()));
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
pub struct HTTPResponse {
    http_response: Vec<u8>,
}

impl HTTPResponse {
    /// Create a new '200 OK' HTTP response.
    pub fn new_200_ok(content: &mut Vec<u8>) -> Self {
        let mut http_response: Vec<u8> = format!("HTTP/1.1 200 OK\r\nContent-Length: {}\r\n\r\n", content.len()).as_bytes().into();
        http_response.append(content);
        Self { http_response }
    }

    /// Create a new '206 Partial Content' HTTP response.
    #[allow(dead_code)] // Only 'write_206_partial_file_to_stream' is actually used in this project, i.e. the more memory-efficient version for sending files.
    pub fn new_206_partial_content(content: &[u8], start_index: &str, end_index: &str) -> Self {
        // cf. https://stackoverflow.com/questions/23071164/grails-ios-specific-returning-video-mp4-file-gives-broken-pipe-exception-g
        let mut http_response: Vec<u8> = format!("HTTP/1.1 206 Partial Content\r\nAccept-Ranges: bytes\r\nContent-Range: bytes {}-{}/{}\r\n\r\n", start_index, end_index, content.len())
            .as_bytes().into();
        http_response.append(&mut content[start_index.parse().unwrap()..=end_index.parse().unwrap()].to_vec()); // Only respond with the requested bytes! "=" because end index in HTTP is inclusive (I think)
        return Self { http_response };
    }

    /// Create a new '401 Unauthorized' HTTP response.
    /// The "Basic" authentication scheme is requested.
    pub fn new_401_unauthorized(realm_name: impl Display) -> Self {
        let http_response: Vec<u8> = format!("HTTP/1.1 401 Unauthorized\r\nWWW-Authenticate: Basic realm=\"{}\"\r\n\r\n", realm_name).as_bytes().into();
        Self { http_response }
    }

    /// Create a new '404 Not Found' HTTP response.
    pub fn new_404_not_found<T: AsRef<str>>(filename: T) -> Self {
        let message = format!("Error: Could not find file {}", filename.as_ref());
        let http_response: Vec<u8> = format!("HTTP/1.1 404 Not Found\r\nContent-Length: {}\r\n\r\n{}", message.len(), message).as_bytes().to_vec();
        Self { http_response }
    }

    /// Create a new '500 Internal Server Error' HTTP response with the given `error_message`.
    pub fn new_500_server_error<T: AsRef<str>>(error_message: T) -> Self {
        let error_message = format!("Internal Server Error occurred: {}", error_message.as_ref());
        let http_response: Vec<u8> = format!("HTTP/1.1 500 Internal Server Error\r\nContent-Length: {}\r\n\r\n{}", error_message.len(), error_message).as_bytes().to_vec();
        Self { http_response }
    }

    /// Directly writes the file contents of `filepath` to `stream`.
    ///
    /// By using file metadata to query the size of the file from the operating system, reading the entire
    /// file into memory only to get its size is avoided, which can save a lot of memory for large files.
    pub fn write_200_ok_file_to_stream(filepath: &Path, stream: &mut TcpStream) -> io::Result<()> {
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

    /// Directly writes the file contents of `filepath` to `stream` in range of bytes from `range`.
    pub fn write_206_partial_file_to_stream(filepath: &Path, range: (u64, Option<u64>), stream: &mut TcpStream) -> io::Result<()> {
        // Try to open the file before writing `206 Partial Content`, so that the HTTP status code can still be
        // changed in case of an error.
        let mut file = File::open(filepath)?;
        // Place read pointer at given start byte
        file.seek(SeekFrom::Start(range.0))?;
        // Only read bytes in given range from file
        let mut partial_file =
            if let Some(range_end) = range.1 { // There is a <range-end> specified:
                file.take(range_end - range.0 + 1) // +1 because end index in HTTP is inclusive!
            } else { // There is no <range-end> specified (e.g. a range of "0-" was requested):
                file.take(u64::MAX) // take all remaining bytes
            };
        // Write http response header
        let file_size: u64 = File::open(filepath)?.metadata()?.len();
        stream.write(format!("HTTP/1.1 206 Partial Content\r\nAccept-Ranges: bytes\r\nContent-Range: bytes {}-{}/{}\r\n\r\n",
                             range.0,
                             range.1.map(|r| r.to_string()).unwrap_or("".to_string()), // None -> ""
                             file_size).as_bytes())?;
        // Write file contents to stream
        io::copy(&mut partial_file, stream)?;
        stream.flush()?;
        Ok(())
    }

    /// Send the created HTTP response to a stream. An IO error may occur, e.g. a "Broken pipe".
    pub fn send_to_tcp_stream(&self, stream: &mut TcpStream) -> std::io::Result<()> {
        // Send the HTTP response to the client:
        stream.write_all(&self.http_response)?;
        stream.flush()?;
        Ok(())
    }
}
