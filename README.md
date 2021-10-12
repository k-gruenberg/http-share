# http-share
**A little Rust app that shares the files of the folder it's put in via HTTP (on port 8080).**

A folder with some files and the *http_share* binary:
![](images/1.png "")

All files (and sub-folders) in a directory are listed as clickable links:
![](images/2.png "")

Viewing a text file:
![](images/3.png "")

All HTTP requests are logged to console:
![](images/4.png "")

Viewing a video file is possible too:
![](images/5.png "")

And sub-directories work too, of course:
![](images/6.png "")

## ToDo's

* ~~**Maybe** add the option to choose the number of columns in *Grid View*.~~
* ~~**Maybe** also show folder size in *List View*.~~
* ~~**Maybe** prettify the HTML <table> borders.~~
* ~~**Maybe** beautify the terminal logging with colors.~~
* Update this README to the newest version (screenshots).