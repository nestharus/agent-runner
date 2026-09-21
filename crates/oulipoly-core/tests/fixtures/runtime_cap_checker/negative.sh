curl --max-time 20 https://example.invalid
curl --connect-timeout=3 https://example.invalid
worker.wait(timeout=4)
