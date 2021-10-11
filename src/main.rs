use percent_encoding::{percent_decode_str, utf8_percent_encode, NON_ALPHANUMERIC};
use std::env;
use std::fs;
use std::io::{self, Error, ErrorKind};
use std::net::{TcpListener, TcpStream};
use std::path::{Path, PathBuf};
use std::thread;
use http_share::{HTTPRequest, HTTPResponse};
use chrono::Local;
use chrono::format::{StrftimeItems, DelayedFormat};
use std::process::Command;
use separator::Separatable;
use chrono::{DateTime, Utc};
use std::time::SystemTime;

fn main() {
    println!(); // separator
    println!("[{}] Starting server...", date_time_str());

    let listener = match TcpListener::bind("0.0.0.0:8080") {
        Ok(listener) => listener,
        Err(err) => {
            eprintln!("[{}] Error: Server could not be started as creating a TCP listener failed: {}", date_time_str(), err);
            return;
        }
    };

    println!("[{}] Server started on {}.", date_time_str(), listener.local_addr().map_or("???".to_string(), |addr| addr.to_string()));

    // Listen for incoming TCP/HTTP connections and handle each of them in a separate thread:
    for stream in listener.incoming() {
        let stream = stream.expect("The iterator returned by incoming() will never return None");

        thread::spawn(|| {
            handle_connection(stream).unwrap_or_else(|err_str| {eprintln!("[{}] Error: {}", date_time_str(), err_str)});
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
    if http_request.contains_range_header() {
        let requested_range = http_request.get_requested_range();
        println!("[{}] {} requested bytes {}-{} of {}",
                 date_time_str(),
                 stream.peer_addr().map_or("???".to_string(), |addr| addr.to_string()),
                 requested_range.0,
                 requested_range.1.map(|r| r.to_string()).unwrap_or("".to_string()),
                 get_path);
    } else {
        println!("[{}] {} requested {}",
                 date_time_str(),
                 stream.peer_addr().map_or("???".to_string(), |addr| addr.to_string()),
                 get_path);
    }

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
        if let Err(err) = file_response(&http_request, fs_path, &mut stream, query_string) {
            HTTPResponse::new_500_server_error(err.to_string());
            return Err(Error::new(ErrorKind::Other, format!("File Response error: {}", err)));
        }
    };
    Ok(())
}

/// Responds to `stream` with the file contents queried by `filepath`.
fn file_response(http_request: &HTTPRequest, filepath: &Path, stream: &mut TcpStream, query_string: Option<&str>) -> io::Result<()> {
    // Check if a thumbnail of a video was requested:
    if let Some("thumbnail") = query_string { // A thumbnail request:
        // 1.) Execute the 'ffmpeg' command to generate a JPEG thumbnail:
        let thumbnail_file_name: &str = &format!("http_share_temp_thumbnail_{}.jpg", filepath.file_name().unwrap_or("".as_ref()).to_str().unwrap_or(""));
        Command::new("ffmpeg")
            .arg("-ss")
            .arg("00:00:01.000")
            .arg("-i")
            .arg(filepath)
            .arg("-vframes")
            .arg("1")
            .arg(thumbnail_file_name)
            .output()?;
        // 2.) Respond with that thumbnail:
        HTTPResponse::write_200_ok_file_to_stream(Path::new(thumbnail_file_name), stream)?;
        // 3.) Immediately delete the temporary thumbnail:
        fs::remove_file(thumbnail_file_name)?;
    } else { // No thumbnail request, respond with a regular file response:
        // Because of iOS we have to differentiate between 2 cases, a normal "full response" and a "range response" (for videos):
        if http_request.contains_range_header() {
            // iOS always requests ranges of video files and expects an according response!:
            // Parse the requested range from the request, so we can create the response for the iOS device:
            HTTPResponse::write_206_partial_file_to_stream(filepath, http_request.get_requested_range(), stream)?;
        } else {
            // The "normal" (either non-video or non-iOS) case, i.e. just return the entire content directly:
            HTTPResponse::write_200_ok_file_to_stream(filepath, stream)?;
        }
    }
    return Ok(());
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
    // Save the number of items (files/directories) in the folder:
    let folder_size: usize = folder_items.len();

    let folder_items = folder_items.iter()
        .map(|path| { format_path(path, query_string) }); // turn the path Strings into HTML links, possibly within a <td>-tag

    let lower_body = match query_string {
        // Grid View (previously called Table View!):
        Some("view=grid") => format!(
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
        // Table View:
        Some("view=table") => format!(
            "<table>\r\n\
            <tr>\
                <th style=\"border: 1px solid black;\">Name</th>\
                <th style=\"border: 1px solid black;\">Size</th>\
                <th style=\"border: 1px solid black;\">Created</th>\
                <th style=\"border: 1px solid black;\">Modified</th>\
                <th style=\"border: 1px solid black;\">Accessed</th>\
            </tr>\
            {}\
            </table>\r\n",
            folder_items.fold(String::from(""), |str1, str2| str1 + &str2)
        ),
        // Default = List View:
        _ => folder_items.fold(String::from(""), |str1, str2| str1 + &str2) // concatenate all the Strings of the iterator together into 1 single String
    };

    // At last, add the "header" (including links/buttons that let the user change the layout):
    return format!( // The leading slash ('/') of the path is added manually, cf. `format_path`.
        "/{} <i>({} items)</i><br>\r\n\
         <a href=\"javascript:window.location.search='view=list';\">List View</a>  |  \r\n\
         <a href=\"javascript:window.location.search='view=table';\">Table View</a>  |  \r\n\
         <a href=\"javascript:window.location.search='view=grid';\">Grid View</a><br>\r\n\
         <hr><br>\r\n\
         {}",
        dir_path, folder_size, lower_body
    );
}

/// A helper function for `format_body`.
/// Formats just the <a>-hyperlinks depending on the layout specified in the `query_string`
/// (either List, Table or Grid View).
fn format_path(path: &String, query_string: Option<&str>) -> String {
    // <a href="hyperlink">display_name</a>
    let hyperlink = utf8_percent_encode(path, NON_ALPHANUMERIC).to_string();
    let display_name = path.split('/').last().unwrap(); // only display the file name to the user

    match query_string {
        // Grid View (previously called Table View!):
        Some("view=grid") => {
            if path.ends_with(".mp4") { // Display ffmpeg generated thumbnails for .mp4 files:
                format!("<td style=\"border: 1px solid black;\"><a href=\"/{}\"><img src=\"/{}?thumbnail\" alt=\"{}\" width=\"100%\"></a></td>\r\n", hyperlink, hyperlink, display_name)
                // Old approach was to show videos in a <video> tag but that was way too computationally expensive:
                // format!("<td style=\"border: 1px solid black;\"><video width=\"100%\" preload=\"metadata\" controls src=\"{}\">{}</video></td>\r\n", hyperlink, display_name)
            } else { // Display all other file types in an HTML <img> Tag with the file name as the alt text:
                format!("<td style=\"border: 1px solid black;\"><a href=\"/{}\"><img src=\"/{}\" alt=\"{}\" width=\"100%\"></a></td>\r\n", hyperlink, hyperlink, display_name)
            }
        },
        // Table View:
        Some("view=table") => {
            // Cf. code in handle_connection()!:
            let binary_path: &String = &env::args().next().expect("Name of binary missing as 0th command line argument");
            let root_dir: &Path = Path::new(binary_path).parent().expect("Binary has no parent");
            let fs_path_buffer: PathBuf = root_dir.join(&path);
            let fs_path: &Path = fs_path_buffer.as_path();

            let metadata = &fs::metadata(fs_path); //File::open(fs_path).unwrap().metadata(); //&fs::metadata(fs_path);
            let metadata = metadata.as_ref();
            format!(
                "<tr>\
                <td style=\"border: 1px solid black;\"><a href=\"/{}\">{}</a></td>\
                <td style=\"border: 1px solid black;\">{}</td>\
                <td style=\"border: 1px solid black;\">{}</td>\
                <td style=\"border: 1px solid black;\">{}</td>\
                <td style=\"border: 1px solid black;\">{}</td>\
                </tr>\r\n",
                hyperlink, display_name,
                metadata.map_or("?".to_string(), |meta|
                    if meta.is_file() {
                        meta.len().separated_string() + "B"
                    } else {
                        format!("<i>({} items)</i>", fs::read_dir(fs_path).map_or("?".to_string(), |dir| dir.count().to_string()))
                    }),
                metadata.map_or("?".to_string(), |meta| system_time_to_string(meta.created())),
                metadata.map_or("?".to_string(), |meta| system_time_to_string(meta.modified())),
                metadata.map_or("?".to_string(), |meta| system_time_to_string(meta.accessed())),
            )
        },
        // Default = List View:
        _ => format!("<a href=\"/{}\">{}</a><br>\r\n", hyperlink, display_name) // The "/" is important!
    }
}

/// Helper function for `format_path`.
fn system_time_to_string(system_time: io::Result<SystemTime>) -> String {
    return match system_time {
        Ok(system_time) =>
            DateTime::<Utc>::from(system_time).format("%Y-%m-%d %H:%M:%S").to_string(),
        Err(_) => "?".to_string()
    };
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

/// Returns the current date/time in the format "%Y-%m-%d %H:%M:%S", for logging to console.
fn date_time_str<'a>() -> DelayedFormat<StrftimeItems<'a>> {
    Local::now().format("%Y-%m-%d %H:%M:%S")
}
