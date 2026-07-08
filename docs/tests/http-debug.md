---
description: "Http request debug using hurl"
---

Use hurl instead of curl or similar commands of powershell to execute real http request. You can use this tool to assist with debugging.

## hurl tool use guideline
This system has setup hurl. You can use it instead of curl or similar tool in powershell.

Hurl is a command line tool that runs HTTP requests defined in a simple plain text format.

It can chain requests, capture values and evaluate queries on headers and body response. Hurl is very versatile: it can be used for both fetching data and testing HTTP sessions.

Hurl makes it easy to work with HTML content, REST / SOAP / GraphQL APIs, or any other XML / JSON based APIs.

Usage: hurl [OPTIONS] [FILES]
Arguments:
  [FILES]...  Set the input file to use

### Use case examples

// Go home and capture token
GET https://example.org
HTTP 200
[Captures]
csrf_token: xpath "string(//meta[@name='_csrf_token']/@content)"


// Do login
POST https://example.org/login
X-CSRF-TOKEN: {{csrf_token}}
[Form]
user: toto
password: 1234
HTTP 302
Chaining multiple requests is easy:

GET https://example.org/api/health
GET https://example.org/api/step1
GET https://example.org/api/step2
GET https://example.org/api/step3


It is well adapted for REST / JSON APIs

POST https://example.org/api/tests
{
    "id": "4568",
    "evaluate": true
}
HTTP 200
[Asserts]
header "X-Frame-Options" == "SAMEORIGIN"
jsonpath "$.status" == "RUNNING"    # Check the status code
jsonpath "$.tests" count == 25      # Check the number of items
jsonpath "$.id" matches /\d{4}/     # Check the format of the id
HTML content

GET https://example.org
HTTP 200
[Asserts]
xpath "normalize-space(//head/title)" == "Hello world!"
GraphQL

POST https://example.org/graphql
```graphql
{
  human(id: "1000") {
    name
    height(unit: FOOT)
  }
}
```
HTTP 200
and even SOAP APIs

POST https://example.org/InStock
Content-Type: application/soap+xml; charset=utf-8
SOAPAction: "http://www.w3.org/2003/05/soap-envelope"
<?xml version="1.0" encoding="UTF-8"?>
<soap:Envelope xmlns:soap="http://www.w3.org/2003/05/soap-envelope" xmlns:m="https://example.org">
  <soap:Header></soap:Header>
  <soap:Body>
    <m:GetStockPrice>
      <m:StockName>GOOG</m:StockName>
    </m:GetStockPrice>
  </soap:Body>
</soap:Envelope>
HTTP 200
Hurl can also be used to test the performance of HTTP endpoints

GET https://example.org/api/v1/pets
HTTP 200
[Asserts]
duration < 1000  # Duration in ms
And check response bytes

GET https://example.org/data.tar.gz
HTTP 200
[Asserts]
sha256 == hex,039058c6f2c0cb492c533b0a4d14ef77cc0f78abccced5287d84a1a2011cfb81;


Usage: hurl.exe [OPTIONS] [FILES]...

Arguments:
  [FILES]...  Set the input file to use

Options:
  -h, --help     Print help
  -V, --version  Print version

HTTP options:
      --aws-sigv4 <PROVIDER1[:PROVIDER2[:REGION[:SERVICE]]]>  Use AWS V4 signature authentication in the transfer
      --cacert <FILE>                                         CA certificate to verify peer against (PEM format)
  -E, --cert <CERTIFICATE[:PASSWORD]>                         Client certificate file and password
      --compressed                                            Request compressed response (using deflate or gzip)
      --connect-timeout <SECONDS>                             Maximum time allowed for connection [default: 300]
      --connect-to <HOST1:PORT1:HOST2:PORT2>                  For a request to the given HOST1:PORT1 pair, connect to HOST2:PORT2 instead
  -H, --header <HEADER>                                       Pass custom header(s) to server
  -0, --http1.0                                               Tell Hurl to use HTTP version 1.0
      --http1.1                                               Tell Hurl to use HTTP version 1.1
      --http2                                                 Tell Hurl to use HTTP version 2
      --http3                                                 Tell Hurl to use HTTP version 3
  -k, --insecure                                              Allow insecure SSL connections
  -4, --ipv4                                                  Tell Hurl to use IPv4 addresses only when resolving host names, and not for example try IPv6
  -6, --ipv6                                                  Tell Hurl to use IPv6 addresses only when resolving host names, and not for example try IPv4
      --key <KEY>                                             Private key file name
      --limit-rate <SPEED>                                    Specify the maximum transfer rate in bytes/second, for both downloads and uploads
  -L, --location                                              Follow redirects
      --location-trusted                                      Follow redirects but allows sending the name + password to all hosts that the site may redirect to
      --max-filesize <BYTES>                                  Specify the maximum size in bytes of a file to download
      --max-redirs <NUM>                                      Maximum number of redirects allowed, -1 for unlimited redirects [default: 50]
  -m, --max-time <SECONDS>                                    Maximum time allowed for the transfer [default: 300]
      --negotiate                                             Tell Hurl to use Negotiate (SPNEGO) authentication
      --noproxy <HOST(S)>                                     List of hosts which do not use proxy
      --ntlm                                                  Tell Hurl to use NTLM authentication
      --path-as-is                                            Tell Hurl to not handle sequences of /../ or /./ in the given URL path
      --pinnedpubkey <HASHES>                                 Public key to verify peer against
  -x, --proxy <[PROTOCOL://]HOST[:PORT]>                      Use proxy on given PROTOCOL/HOST/PORT
      --resolve <HOST:PORT:ADDR>                              Provide a custom address for a specific HOST and PORT pair
      --ssl-no-revoke                                         (Windows) Tell Hurl to disable certificate revocation checks
      --unix-socket <PATH>                                    (HTTP) Connect through this Unix domain socket, instead of using the network
  -u, --user <USER:PASSWORD>                                  Add basic Authentication header to each request
  -A, --user-agent <NAME>                                     Specify the User-Agent string to send to the HTTP server

Output options:
      --color                  Colorize output
      --curl <FILE>            Export each request to a list of curl commands
      --error-format <FORMAT>  Control the format of error messages [default: short] [possible values: short, long]
  -i, --include                Include the HTTP headers in the output
      --json                   Output each Hurl file result to JSON
      --no-color               Do not colorize output
      --no-output              Suppress output. By default, Hurl outputs the body of the last response
  -o, --output <FILE>          Write to FILE instead of stdout
      --progress-bar           Display a progress bar in test mode
  -v, --verbose                Turn on verbose
      --very-verbose           Turn on verbose output, including HTTP response and libcurl logs

Run options:
      --continue-on-error              Continue executing requests even if an error occurs
      --delay <MILLISECONDS>           Sets delay before each request (aka sleep) [default: 0]
      --from-entry <ENTRY_NUMBER>      Execute Hurl file from ENTRY_NUMBER (starting at 1)
      --ignore-asserts                 Ignore asserts defined in the Hurl file
      --jobs <NUM>                     Maximum number of parallel jobs, 0 to disable parallel execution
      --parallel                       Run files in parallel (default in test mode)
      --repeat <NUM>                   Repeat the input files sequence NUM times, -1 for infinite loop
      --retry <NUM>                    Maximum number of retries, 0 for no retries, -1 for unlimited retries
      --retry-interval <MILLISECONDS>  Interval in milliseconds before a retry [default: 1000]
      --secret <NAME=VALUE>            Define a variable which value is secret
      --test                           Activate test mode (use parallel execution)
      --to-entry <ENTRY_NUMBER>        Execute Hurl file to ENTRY_NUMBER (starting at 1)
      --variable <NAME=VALUE>          Define a variable
      --variables-file <FILE>          Define a properties file in which you define your variables

Report options:
      --report-html <DIR>    Generate HTML report to DIR
      --report-json <DIR>    Generate JSON report to DIR
      --report-junit <FILE>  Write a JUnit XML report to FILE
      --report-tap <FILE>    Write a TAP report to FILE

Other options:
  -b, --cookie <FILE>      Read cookies from FILE
  -c, --cookie-jar <FILE>  Write cookies to FILE after running the session
      --file-root <DIR>    Set root directory to import files [default: input file directory]
      --glob <GLOB>        Specify input files that match the given GLOB. Multiple glob flags may be used
  -n, --netrc              Must read .netrc for username and password
      --netrc-file <FILE>  Specify FILE for .netrc
      --netrc-optional     Use either .netrc or the URL


### Advantage
Text Format: for both devops and developers
Fast CLI: a command line for local dev and continuous integration
Single Binary: easy to install, with no runtime required
Powered by curl
Hurl is a lightweight binary written in Rust. Under the hood, Hurl HTTP engine is powered by libcurl, one of the most powerful and reliable file transfer libraries. With its text file format, Hurl adds syntactic sugar to run and test HTTP requests, but it's still the curl that we love: fast, efficient and IPv6 / HTTP/3 ready.


## Step

1. Based on my feedback analysis, identifying potential causes of the issue requires verification using actual HTTP connections.
2. Create hurl folder, and add hurl files for each test case.
3. Create documentation in the docs directory: `HTTP-debug-archive.md`. Include the HURL commands you need to use, expected results, and related issues.
4. Execute the commands, briefly describe the actual results and any issues observed, and insert this information into the archive document.