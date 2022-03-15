use std::collections::HashMap;
use percent_encoding::{percent_decode_str, utf8_percent_encode, NON_ALPHANUMERIC};
use std::env;
use std::fs;
use std::io::{self, Error, ErrorKind, Write};
use std::net::{TcpListener, TcpStream};
use std::path::{Path, PathBuf};
use std::thread;
use http_share::{HTTPRequest, HTTPResponse};
use chrono::Local;
use chrono::format::{StrftimeItems, DelayedFormat};
use std::process::Command;
use std::sync::Mutex;
use separator::Separatable;
use chrono::{DateTime, Utc};
use std::time::SystemTime;
use ansi_term::Colour::Red;
use lazy_static::lazy_static;
use rand::thread_rng;
use rand::seq::SliceRandom;

fn main() {
    println!(); // separator
    
    println!("Please provide credentials or hit ENTER two times to not use any authorization:");
    print!("Username: ");
    io::stdout().flush().unwrap();
    let mut username = String::new();
    io::stdin().read_line(&mut username).unwrap();
    username = username.trim().to_string(); // Trimming is done mainly to get rid of the newline at the end.
    print!("Password: ");
    io::stdout().flush().unwrap();
    let mut password = String::new();
    io::stdin().read_line(&mut password).unwrap();
    password = password.trim().to_string();
    if username != "" || password != "" {
        println!("Credentials set to: Username: \"{}\" & Password: \"{}\"", username, password);
    } else {
        println!("No credentials set.");
    }

    println!(); // separator
    
    println!("[{}] Starting server...", date_time_str());

    let mut port = 8080; // default port
    let listener: TcpListener;
    loop {
        if port > 8180 { // Stop trying out ports after reaching 8180:
            eprintln!("{}", Red.paint(format!("[{}] Error: Server was not started because ports 8080 - 8180 are all already in use!", date_time_str())));
            return;
        }
        match TcpListener::bind(format!("0.0.0.0:{}", port)) {
            Ok(tcp_listener) => {
                listener = tcp_listener;
                break;
            },
            Err(err) => {
                if err.to_string().contains("Address already in use") {
                    port += 1;
                    continue;
                } else {
                    eprintln!("{}", Red.paint(format!("[{}] Error: Server could not be started as creating a TCP listener failed: {}", date_time_str(), err)));
                    return;
                }
            }
        }
    }

    /*
    // Version that only tries out port 8080:
    let listener = match TcpListener::bind("0.0.0.0:8080") {
        Ok(listener) => listener,
        Err(err) => {
            eprintln!("{}", Red.paint(format!("[{}] Error: Server could not be started as creating a TCP listener failed: {}", date_time_str(), err)));
            return;
        }
    };
     */

    println!("[{}] Server started on {}.", date_time_str(), listener.local_addr().map_or("???".to_string(), |addr| addr.to_string()));

    // Listen for incoming TCP/HTTP connections and handle each of them in a separate thread:
    for stream in listener.incoming() {
        let stream = stream.expect("The iterator returned by incoming() will never return None");

        let username = username.clone(); // https://github.com/rust-lang/rust/issues/41851#issuecomment-332276034
        let password = password.clone();
        thread::spawn(move || {
            let ip_addr: String = stream.peer_addr().map_or("???".to_string(), |addr| addr.to_string());
            handle_connection(stream, username, password).unwrap_or_else(
                |err_str| {eprintln!("{}", Red.paint(format!("[{}] Error while serving {}: {}", date_time_str(), ip_addr, err_str)))}
            );
        });
    }
}

/// Handles a connection coming from `stream`.
/// When `username != "" || password != ""` it also checks whether the correct `username` and
/// `password` were provided – if not, it responds with a '401 Unauthorized'.
fn handle_connection(mut stream: TcpStream, username: String, password: String) -> std::io::Result<()> {
    // Read and parse the HTTP request:
    let http_request: HTTPRequest = match HTTPRequest::read_from_tcp_stream(&mut stream) {
        Ok(http_request) => http_request,
        Err(_err) => {
            HTTPResponse::new_500_server_error("Could not read HTTP request").send_to_tcp_stream(&mut stream)?;
            return Err(Error::new(ErrorKind::Other, "TCP stream could not be read!"));
        }
    };
    let get_path: &str = http_request.get_get_path();

    // Do the HTTP Auth check:
    if username != "" || password != "" { // A username and password are necessary, i.e. auth protection is turned on:
        match http_request.get_authorization() {
            Some((provided_uname, provided_pw))
              if provided_uname == username && provided_pw == password => {}, // Uname & PW ok, do nothing and continue...
            Some((provided_uname, provided_pw)) => { // An invalid authorization was provided:
                HTTPResponse::new_401_unauthorized("").send_to_tcp_stream(&mut stream)?;
                return Err(Error::new(ErrorKind::Other, format!("requested {} with incorrect credentials: {}:{}", get_path, provided_uname, provided_pw)));
            }
            None => { // No authorization was provided:
                HTTPResponse::new_401_unauthorized("").send_to_tcp_stream(&mut stream)?;
                return Err(Error::new(ErrorKind::Other, format!("requested {} without giving credentials!", get_path)));
            }
        }
    }

    // Sanity check the requested GET path for security reasons:
    if !get_path.starts_with('/') {
        HTTPResponse::new_500_server_error("GET path does not start with a '/'!").send_to_tcp_stream(&mut stream)?;
        return Err(Error::new(ErrorKind::Other, format!("requested {} which does not start with a '/'!", get_path)));
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

lazy_static! {
    /// Cached JPEG thumbnails of video files whose thumbnail was already requested before.
    static ref CACHED_THUMBNAILS: Mutex<HashMap<PathBuf, Vec<u8>>> = Mutex::new(HashMap::new());
}

/// Responds to `stream` with the file contents queried by `filepath`.
fn file_response(http_request: &HTTPRequest, filepath: &Path, stream: &mut TcpStream, query_string: Option<&str>) -> io::Result<()> {
    // Check if a thumbnail of a video was requested:
    if let Some("thumbnail") = query_string { // A thumbnail request:
        HTTPResponse::new_200_ok(
            &mut CACHED_THUMBNAILS.lock().unwrap()
                .entry(PathBuf::from(filepath))
                .or_insert_with(|| generate_jpeg_thumbnail(filepath))
                .clone() // Cloning is necessary because `new_200_ok` mutates the Vec it's given, emptying it!!
        ).send_to_tcp_stream(stream)?;
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

/// A helper function for `file_response`.
/// Takes a path to a video file and returns a JPEG thumbnail preview of it.
/// It generates such a thumbnail by executing the "ffmpeg" command in console.
fn generate_jpeg_thumbnail(video_file_path: &Path) -> Vec<u8> {
    // 0.) The name/location of the temporary JPEG thumbnail file:
    let thumbnail_file_name: &str =
        &format!("http_share_temp_thumbnail_{}.jpg",
                 video_file_path.file_name().unwrap_or("".as_ref()).to_str().unwrap_or(""));

    // 1.) Execute the 'ffmpeg' command to generate a JPEG thumbnail to said location:
    if let Err(err) = Command::new("ffmpeg")
        .arg("-ss")
        .arg("00:00:01.000")
        .arg("-i")
        .arg(video_file_path)
        .arg("-vframes")
        .arg("1")
        .arg(thumbnail_file_name)
        .output() {
            eprintln!("{}", Red.paint(format!(
                "[{}] Error: Failed to generate thumbnail file '{}' with ffmpeg! Error message: {}",
                date_time_str(), thumbnail_file_name, err)));
    }

    // 2.) Read the file generated by the "ffmpeg" command into memory:
    let result: Vec<u8> = fs::read(thumbnail_file_name).unwrap_or_else(
        |err| {
            eprintln!("{}", Red.paint(format!(
                "[{}] Error: Failed to read generated thumbnail file '{}' Error message: {}",
                date_time_str(), thumbnail_file_name, err)));
            Vec::new()
        }
    );

    // 3.) Delete the temporary thumbnail file:
    if let Err(err) = fs::remove_file(thumbnail_file_name) {
        eprintln!("{}", Red.paint(format!(
            "[{}] Error: Failed to delete temporary file '{}' Please delete it manually! Error message: {}",
            date_time_str(), thumbnail_file_name, err)));
    }

    println!("[{}] Generated {} byte JPEG thumbnail for {}", date_time_str(), result.len(), video_file_path.display());

    // 4.) Return the file content read in step 2.):
    return result;
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
        // Take care of the sorting preference / the "sort=..." URL GET parameter:
        match query_string
            .map(|query_str| query_str.split("&").find(|param| param.starts_with("sort=")))
            .flatten()
        {
            // Sort Descending: Display the folder items in reverse alphabetical order (but case-insensitive!):
            Some("sort=desc") => folder_items.sort_by(|a,b| b.to_lowercase().cmp(&a.to_lowercase())),
            // Sort Randomly:
            Some("sort=rand") => folder_items.shuffle(&mut thread_rng()),
            // Default = Sort Ascending: Display the folder items in alphabetical order (but case-insensitive!):
            _ => folder_items.sort_by(|a,b| a.to_lowercase().cmp(&b.to_lowercase()))
        }

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

    let lower_body = match query_string
            .map(|query_str| query_str.split("&").find(|param| param.starts_with("view=")))
            .flatten()
    {
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
            "<table id=\"tableViewTable\">\r\n\
            <tr>\
                <th onclick=\"sortTable(0, x => x)\" style=\"border: 1px solid black;\">Name</th>\
                <th onclick=\"sortTable(1, x => parseInt(x.replaceAll(',','')) || 0)\" style=\"border: 1px solid black;\">Size</th>\
                <th onclick=\"sortTable(2, x => x)\" style=\"border: 1px solid black;\">Created</th>\
                <th onclick=\"sortTable(3, x => x)\" style=\"border: 1px solid black;\">Modified</th>\
                <th onclick=\"sortTable(4, x => x)\" style=\"border: 1px solid black;\">Accessed</th>\
            </tr>\
            {}\
            </table>\r\n{}\r\n",
            folder_items.fold(String::from(""), |str1, str2| str1 + &str2),
            SORT_TABLE_JAVASCRIPT
        ),
        // Default = List View:
        _ => folder_items.fold(String::from(""), |str1, str2| str1 + &str2) // concatenate all the Strings of the iterator together into 1 single String
    };

    // At last, add the "header" (including links/buttons that let the user change the layout):
    return format!( // The leading slash ('/') of the path is added manually, cf. `format_path`.
        "/{} <i>({} items)</i><br>\r\n\
         <script>\
             function setURLSearchParams(view, sort) {{ \
                 if (view == null) {{ /* ...then use current value... */
                     view = window.location.search.split('&').filter(param => param.includes('view='))[0]?.split('=')[1];
                 }}
                 if (view == null) {{ /* ...or else the default value: */
                     view = 'list';
                 }}
                 if (sort == null) {{ /* ...then use current value... */
                     sort = window.location.search.split('&').filter(param => param.includes('sort='))[0]?.split('=')[1];
                 }}
                 if (sort == null) {{ /* ...or else the default value: */
                     sort = 'asc';
                 }}
                 window.location.search = '?view=' + view + '&sort=' + sort;\
             }}\
         </script>\
         <a href=\"javascript:setURLSearchParams('list', null);\">List View</a>  |  \r\n\
         <a href=\"javascript:setURLSearchParams('table', null);\">Table View</a>  |  \r\n\
         <a href=\"javascript:setURLSearchParams('grid', null);\">Grid View</a><br>\r\n\
         Sort: <a href=\"javascript:setURLSearchParams(null, 'asc');\">Ascending</a>  |  \r\n\
         <a href=\"javascript:setURLSearchParams(null, 'desc');\">Descending</a>  |  \r\n\
         <a href=\"javascript:setURLSearchParams(null, 'rand');\">Randomly</a><br>\r\n\
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

    match query_string
        .map(|query_str| query_str.split("&").find(|param| param.starts_with("view=")))
        .flatten()
    {
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

/// Returns the current date/time in the format "%Y-%m-%d %H:%M:%S", for logging to console.
fn date_time_str<'a>() -> DelayedFormat<StrftimeItems<'a>> {
    Local::now().format("%Y-%m-%d %H:%M:%S")
}

// Source: https://www.w3schools.com/howto/howto_js_sort_table.asp
const SORT_TABLE_JAVASCRIPT: &str =
"<!-- Script below taken (and slightly adapted) from: https://www.w3schools.com/howto/howto_js_sort_table.asp -->
<script>
function sortTable(n, apply_before) {
  var table, rows, switching, i, x, y, shouldSwitch, dir, switchcount = 0;
  table = document.getElementById(\"tableViewTable\");
  switching = true;
  // Set the sorting direction to ascending:
  dir = \"asc\";
  /* Make a loop that will continue until
  no switching has been done: */
  while (switching) {
    // Start by saying: no switching is done:
    switching = false;
    rows = table.rows;
    /* Loop through all table rows (except the
    first, which contains table headers): */
    for (i = 1; i < (rows.length - 1); i++) {
      // Start by saying there should be no switching:
      shouldSwitch = false;
      /* Get the two elements you want to compare,
      one from current row and one from the next: */
      x = rows[i].getElementsByTagName(\"TD\")[n];
      y = rows[i + 1].getElementsByTagName(\"TD\")[n];
      /* Check if the two rows should switch place,
      based on the direction, asc or desc: */
      if (dir == \"asc\") {
        if (apply_before(x.innerHTML.toLowerCase()) > apply_before(y.innerHTML.toLowerCase())) {
          // If so, mark as a switch and break the loop:
          shouldSwitch = true;
          break;
        }
      } else if (dir == \"desc\") {
        if (apply_before(x.innerHTML.toLowerCase()) < apply_before(y.innerHTML.toLowerCase())) {
          // If so, mark as a switch and break the loop:
          shouldSwitch = true;
          break;
        }
      }
    }
    if (shouldSwitch) {
      /* If a switch has been marked, make the switch
      and mark that a switch has been done: */
      rows[i].parentNode.insertBefore(rows[i + 1], rows[i]);
      switching = true;
      // Each time a switch is done, increase this count by 1:
      switchcount ++;
    } else {
      /* If no switching has been done AND the direction is \"asc\",
      set the direction to \"desc\" and run the while loop again. */
      if (switchcount == 0 && dir == \"asc\") {
        dir = \"desc\";
        switching = true;
      }
    }
  }
}
</script>";
