function done(summary, latency, requests)
  io.write(string.format("LATENCY p95_us=%.0f p99_us=%.0f\n",latency:percentile(95),latency:percentile(99)))
end
