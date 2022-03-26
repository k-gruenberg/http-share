use std::collections::HashMap;
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
    ///
    /// This only works with the 'Basic' authentication scheme!
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


    /// Verify the Authorization provided by the client in the "Authorization" request header.
    /// Returns `Ok(true)` when the client successfully authorized itself.
    /// Returns `Ok(false)` when the client provided no or an incorrect Authorization.
    /// Returns `Err` when either an incomplete or an incorrectly formatted (syntax) Authorization was provided.
    ///
    /// This only works with the 'Digest' authentication scheme!
    ///
    /// The `nonce_opaque_verifier` takes the server nonce as returned by the client as its 1st
    /// argument and the server's opaque as returned by the client as its 2nd argument.
    /// It shall verify that the nonce actually came from the server and that it is not too old,
    /// i.e. expired. One may also check whether it was intended for the correct ip address.
    /// A common way to do that is to choose the *opaque* as an HMAC of the server *nonce*.
    ///
    /// When `opaque` is set to `None` it is not verified whether the client responded with
    /// the same 'opaque' value in its request header or even whether the client gave an 'opaque'
    /// value in its request header at all.
    /// When `opaque` is set to `Some` and the client responded either with no or with a different
    /// 'opaque' value in its request header, this functions returns `Some(false)` even when the client
    /// otherwise correctly identified itself!
    ///
    /// When `last_counter` ist set to `Some` it is ensured that the hexadecimal counter (nc)
    /// of this request is strictly larger! This is to prevent replay attacks.
    /// This also means that the usage of RFC 2617 instead of the old RFC 2069 is required.
    /// When `last_counter` ist set to `None` no such check is performed and an attacker could
    /// request the same site/URI with the same credentials again.
    /// This should only be a security issue for non-static websites.
    /// When `last_counter` ist set to `None`, the legacy RFC 2069 may be used.
    ///
    /// Integrity protection ("auth-int") is currently **not** supported/checked!
    pub fn verify_digest_authorization<F>(&self, username: &str, password: impl Display, realm: &str, nonce_opaque_verifier: F, last_counter: Option<u128>) -> Result<bool, String>
        where F: Fn(&str, &str) -> bool
    {
        /*
        Example of a client request with username "Mufasa" and password "Circle Of Life"
        (from https://en.wikipedia.org/wiki/Digest_access_authentication#Example_with_explanation):

        GET /dir/index.html HTTP/1.0
        Host: localhost
        Authorization: Digest username="Mufasa",
                             realm="testrealm@host.com",
                             nonce="dcd98b7102dd2f0e8b11d0f600bfb0c093",
                             uri="/dir/index.html",
                             qop=auth,
                             nc=00000001,
                             cnonce="0a4f113b",
                             response="6629fae49393a05397450978507c4ef1",
                             opaque="5ccc069c403ebaf9f0171e9517f40e41"
         */

        if !self.http_request.contains("Authorization: Digest ") {
            return Ok(false); // the client provided no (digest) Authorization at all.
        }

        // 1.) parse the key value pairs provided in the Authorization HTTP header into a HashMap:
        let given_key_value_pairs: HashMap<&str, &str> = self.http_request
            .split("Authorization: Digest ")
            .nth(1).ok_or("client's request header does not contain substring 'Authorization: Digest '")? // should never occur/always succeed due to check above
            .split(",")
            .map(|key_value_pair| key_value_pair.trim())
            .map(|kv_pair| (kv_pair.split("=").nth(0).unwrap_or(""), kv_pair.split("=").nth(1).unwrap_or("")))
            .map(|(key, value)| (key, value.strip_prefix("\"").map(|v| v.strip_suffix("\"")).flatten().unwrap_or(value)))
            .collect();

        // 2.) put all the values of interest into separate variables:
        let given_username: &str = given_key_value_pairs.get("username").ok_or("client specified no 'username' in Authorization header field")?;
        let given_realm: &str = given_key_value_pairs.get("realm").ok_or("client specified no 'realm' in Authorization header field")?;
        let given_nonce: &str = given_key_value_pairs.get("nonce").ok_or("client specified no 'nonce' in Authorization header field")?;
        let given_uri: &str = given_key_value_pairs.get("uri").ok_or("client specified no 'uri' in Authorization header field")?;
        let given_qop: Option<&&str> = given_key_value_pairs.get("qop"); // qop was only added with RFC 2617, therefore it's optional
        let given_nc: Option<&&str> = given_key_value_pairs.get("nc"); // nonce counter was only added with RFC 2617, therefore it's optional
        let given_cnonce: Option<&&str> = given_key_value_pairs.get("cnonce"); // client-generated random nonce was only added with RFC 2617, therefore it's optional
        let given_response: &str = given_key_value_pairs.get("response").ok_or("client specified no 'response' in Authorization header field")?;
        let given_opaque: &str = given_key_value_pairs.get("opaque").ok_or("client specified no 'opaque' in Authorization header field")?;

        // 3.) verify some of the given values:
        if given_username != username || given_realm != realm {
            return Ok(false); // reject authorizations for the incorrect username or realm
        }
        if !nonce_opaque_verifier(given_nonce, given_opaque) {
            return Ok(false); // reject incorrect nonce's (correctness of the nonce is verified using the opaque value)
        }
        if given_uri != self.get_get_path() {
            return Ok(false);
        }
        if last_counter != None && (given_nc == None || u128::from_str_radix(given_nc.unwrap(), 16).ok().ok_or("could not parse 'nc' to an int")? <= last_counter.unwrap()) {
            return Ok(false); // request counter (nc) not strictly increasing (or not even provided)! replay attack detected!
        }

        // 4.) compute the expected value/md5 hash for the "response" value:
        let ha1 = md5::compute(format!("{}:{}:{}", username, realm, password));
        let ha2 = md5::compute(format!("GET:{}", self.get_get_path()));
        let expected_response =
        if given_qop.is_some() && given_nc.is_some() && given_cnonce.is_some() { // new RFC 2617:
            md5::compute(
                format!("{:x}:{}:{}:{}:{}:{:x}", ha1, given_nonce, given_nc.unwrap(), given_cnonce.unwrap(), given_qop.unwrap(), ha2)
            )
        } else if given_qop.is_none() && given_nc.is_none() && given_cnonce.is_none() { // old RFC 2069:
            // Note when last_counter.is_some() this piece of code is unreachable!!
            md5::compute(
                format!("{:x}:{}:{:x}", ha1, given_nonce, ha2)
            )
        } else {
            return Err(String::from("an invalid mix between the old RFC 2069 and the new RFC 2617: qop, nc, cnonce are only partially specified"));
        };
        let expected_response_hex = format!("{:x}", expected_response); // to hexadecimal

        // 5.) compare the expected "response" value to the value actually given and return the result as a bool:
        return Ok(given_response == expected_response_hex);

        /*
        From https://en.wikipedia.org/wiki/Digest_access_authentication#Example_with_explanation:

        The "response" value is calculated in three steps, as follows. Where values are combined, they are delimited by colons.

        1. The MD5 hash of the combined username, authentication realm and password is calculated.
           The result is referred to as HA1.
        2. The MD5 hash of the combined method and digest URI is calculated, e.g. of "GET" and
           "/dir/index.html". The result is referred to as HA2.
        3. The MD5 hash of the combined HA1 result, server nonce (nonce), request counter (nc),
           client nonce (cnonce), quality of protection code (qop) and HA2 result is calculated.
           The result is the "response" value provided by the client.

        Since the server has the same information as the client, the response can be checked by
        performing the same calculation. In the example given above the result is formed as follows,
        where MD5() represents a function used to calculate an MD5 hash, backslashes represent a
        continuation and the quotes shown are not used in the calculation.

        Completing the example given in RFC 2617 gives the following results for each step.

        HA1 = MD5( "Mufasa:testrealm@host.com:Circle Of Life" )
            = 939e7578ed9e3c518a452acee763bce9

        HA2 = MD5( "GET:/dir/index.html" )
            = 39aff3a2bab6126f332b942af96d3366

        Response = MD5( "939e7578ed9e3c518a452acee763bce9:\
                         dcd98b7102dd2f0e8b11d0f600bfb0c093:\
                         00000001:0a4f113b:auth:\
                         39aff3a2bab6126f332b942af96d3366" )
                 = 6629fae49393a05397450978507c4ef1
         */
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

    /// Create a new '400 Bad Request' HTTP response.
    pub fn new_400_bad_request(content: &mut Vec<u8>) -> Self {
        let mut http_response: Vec<u8> = format!("HTTP/1.1 400 Bad Request\r\nContent-Length: {}\r\n\r\n", content.len()).as_bytes().into();
        http_response.append(content);
        Self { http_response }
    }

    /// Create a new '401 Unauthorized' HTTP response.
    /// The "Basic" authentication scheme is requested.
    pub fn new_401_unauthorized(realm_name: impl Display) -> Self {
        let http_response: Vec<u8> = format!("HTTP/1.1 401 Unauthorized\r\nWWW-Authenticate: Basic realm=\"{}\"\r\n\r\n", realm_name).as_bytes().into();
        Self { http_response }
    }

    /// Create a new '401 Unauthorized' HTTP response.
    /// The "Digest" authentication scheme is requested.
    /// The `nonce` is the challenge to the client to authenticate itself.
    /// `opaque` is a server-specified string that shall be returned unchanged in the Authorization
    /// header by the client.
    ///
    /// The `qop_auth` and `qop_auth_int` parameters control the quality of protection (qop).
    /// "auth-int" stands for *Authentication with integrity protection*.
    /// When both are set to false, the qop directive is unspecified and the legacy RFC 2069
    /// will be used. Otherwise, the newer RFC 2617 will be used.
    /// RFC 2617 adds "quality of protection" (qop), nonce counter incremented by client,
    /// and a client-generated random nonce.
    pub fn new_401_unauthorized_digest(realm_name: impl Display, nonce: impl Display, opaque: impl Display, qop_auth: bool, qop_auth_int: bool) -> Self {
        // cf. https://en.wikipedia.org/wiki/Digest_access_authentication#Example_with_explanation
        // and https://developer.mozilla.org/en-US/docs/Web/HTTP/Headers/WWW-Authenticate
        let http_response: Vec<u8> = format!(
            "HTTP/1.1 401 Unauthorized\r\n\
            WWW-Authenticate: Digest realm=\"{}\",\r\n\
                                    {}\
                                    nonce=\"{}\",\r\n\
                                    opaque=\"{}\"\r\n\
            \r\n",
            realm_name,
            match (qop_auth, qop_auth_int) {
                (true, true) => "qop=\"auth,auth-int\",\r\n",
                (true, false) => "qop=\"auth\",\r\n",
                (false, true) => "qop=\"auth-int\",\r\n",
                (false, false) => ""
            },
            nonce,
            opaque
        ).as_bytes().into();
        Self { http_response }
    }

    /// Create a new '403 Forbidden' HTTP response.
    pub fn new_403_forbidden(content: &mut Vec<u8>) -> Self {
        let mut http_response: Vec<u8> = format!("HTTP/1.1 403 Forbidden\r\nContent-Length: {}\r\n\r\n", content.len()).as_bytes().into();
        http_response.append(content);
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
