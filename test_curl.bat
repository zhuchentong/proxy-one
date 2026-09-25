@echo off
echo [1] HTTPS via HTTP-proxy CONNECT:
curl.exe -x http://127.0.0.1:8888 -s -o NUL -w "code=%%{http_code} time=%%{time_total}s\n" --max-time 15 https://www.google.com
echo.
echo [2] Plain HTTP absolute-URI:
curl.exe -x http://127.0.0.1:8888 -s -o NUL -w "code=%%{http_code} time=%%{time_total}s\n" --max-time 15 http://www.gstatic.com/generate_204
echo.
echo [3] SOCKS5:
curl.exe -x socks5h://127.0.0.1:8888 -s -o NUL -w "code=%%{http_code} time=%%{time_total}s\n" --max-time 15 https://www.google.com
echo.
echo [4] SOCKS5 plain HTTP:
curl.exe -x socks5h://127.0.0.1:8888 -s -o NUL -w "code=%%{http_code} time=%%{time_total}s\n" --max-time 15 http://www.gstatic.com/generate_204
echo.
