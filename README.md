# http-share
**A Rust app that shares the files of the folder it's put in via HTTP (on port 8080).**

## Features

* Memory-efficient video streaming.
* Protect the server with a custom username and password (HTTP Basic Authentication).
* View the files in a folder in 3 different views/layouts: a basic *List View*, *Table View* (sortable columns!) and *Grid View* (pictures shown, thumbnails for videos).
* *ffmpeg*-generated thumbnails for .mp4 files in *Grid View*.
* All (HTTP) requests, wrong password attempts and errors are logged to console with timestamp and IP address (the latter two in red color for emphasis).
* Unicode/UTF-8 support for file/folder names.
* Tested on iOS (support for HTTP range requests).

## Screenshots

A folder with some files and the *http_share* binary:
![](images/example_folder.png "")

The terminal output of the started *http_share* binary: all HTTP requests are logged to console with timestamp and IP address:
![](images/console_log.png "")

List View (default):
![](images/list_view.png "")
/Users/kendrick/Documents/Github_k-gruenberg/http-share
Table View:
![](images/table_view.png "")

Sorting by a column in Table View, e.g. ascending by size:
![](images/table_view_sorted.png "")

Grid View:
![](images/grid_view.png "")

Viewing a text file:
![](images/viewing_a_text_file.png "")

Playing a video file:
![](images/playing_a_video_file.png "")

Subfolders work too, of course, the current location/directory is shown at the very top:
![](images/subfolders.png "")

HTTP Basic Authentication:
![](images/http_basic_auth.png "")

## ToDo's

* Include IP address in 'File Response error' and 'Directory Response error' messages.