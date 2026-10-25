require_relative 'spec_helper'
require 'net/http'
require 'uri'
require 'socket'
require 'json'

RSpec.describe 'Proxy HTTP Traffic Recording', :with_fiddler do
  # In-process lightweight HTTP mock server using standard library TCPServer
  class MockHttpServer
    attr_reader :port, :thread

    def initialize(port = 0)
      @server = TCPServer.new('127.0.0.1', port)
      @port = @server.addr[1]
      @running = false
    end

    def start
      @running = true
      @thread = Thread.new do
        while @running
          begin
            client = @server.accept
          rescue IOError
            break
          end
          handle_client(client)
        end
      end
    end

    def stop
      @running = false
      @server.close rescue nil
      @thread.join(1) rescue nil
    end

    private

    def handle_client(client)
      Thread.new(client) do |socket|
        request_lines = []
        while (line = socket.gets)
          break if line.strip.empty?
          request_lines << line
        end

        first_line = request_lines.first || ''
        method, path, = first_line.split(' ')

        content_length = 0
        request_lines.each do |hdr|
          if hdr =~ /^Content-Length:\s*(\d+)/i
            content_length = $1.to_i
          end
        end
        body = content_length > 0 ? socket.read(content_length) : ''

        response_body = case path
                        when '/hello'
                          'Hello from mock origin'
                        when '/echo_json'
                          JSON.generate({ received: body, method: method })
                        when '/status/404'
                          'Not Found'
                        else
                          'OK'
                        end

        status_code = path == '/status/404' ? '404 Not Found' : '200 OK'
        content_type = path == '/echo_json' ? 'application/json' : 'text/plain'

        socket.print "HTTP/1.1 #{status_code}\r\n"
        socket.print "Content-Type: #{content_type}\r\n"
        socket.print "Content-Length: #{response_body.bytesize}\r\n"
        socket.print "Connection: close\r\n"
        socket.print "X-Mock-Header: fiddler-test\r\n"
        socket.print "\r\n"
        socket.print response_body
        socket.close
      end
    end
  end

  let!(:mock_server) { MockHttpServer.new(0) }
  let(:proxy_host) { '127.0.0.1' }
  let(:proxy_port) { 8897 }

  before do
    mock_server.start
  end

  after do
    mock_server.stop
  end

  def send_proxied_request(method, path, body: nil, headers: {})
    uri = URI("http://127.0.0.1:#{mock_server.port}#{path}")
    proxy = Net::HTTP::Proxy(proxy_host, proxy_port)
    req_class = case method
                when :get then Net::HTTP::Get
                when :post then Net::HTTP::Post
                when :put then Net::HTTP::Put
                when :delete then Net::HTTP::Delete
                else Net::HTTP::Get
                end

    req = req_class.new(uri.request_uri)
    headers.each { |k, v| req[k] = v }
    req.body = body if body

    proxy.start(uri.host, uri.port) do |http|
      http.read_timeout = 5
      http.request(req)
    end
  end

  def wait_for_session(expected_count: 1, timeout: 5)
    start_time = Time.now
    loop do
      sessions = fiddler.read_sessions
      return sessions if sessions.size >= expected_count
      raise Timeout::Error, "Did not capture #{expected_count} session(s) in #{timeout}s" if Time.now - start_time > timeout
      sleep 0.1
    end
  end

  it 'captures GET request and response with complete schema validation' do
    res = send_proxied_request(:get, '/hello', headers: { 'User-Agent' => 'FiddlerSpec/1.0', 'X-Custom-Req' => 'spec-val' })
    expect(res.code).to eq('200')
    expect(res.body).to eq('Hello from mock origin')

    sessions = wait_for_session(expected_count: 1)
    session = sessions.last

    # Validate SessionRecord schema structure
    expect(session).to include(
      'id',
      'method',
      'url',
      'status',
      'http_version',
      'response_http_version',
      'request_headers',
      'response_headers',
      'request_body',
      'response_body',
      'start_time',
      'duration_ms'
    )

    expect(session['id']).to be_a(Integer)
    expect(session['method']).to eq('GET')
    expect(session['url']).to eq("http://127.0.0.1:#{mock_server.port}/hello")
    expect(session['status']).to eq(200)
    expect(session['http_version']).to match(/HTTP\/1\.[01]/)
    expect(session['response_http_version']).to match(/HTTP\/1\.[01]/)
    expect(session['response_body']).to eq('Hello from mock origin')
    expect(session['duration_ms']).to be_a(Integer).and be >= 0
    expect(session['start_time']).to match(/\d{4}-\d{2}-\d{2}T/)

    # Headers verification
    req_headers = session['request_headers']
    expect(req_headers['x-custom-req'] || req_headers['X-Custom-Req']).to eq('spec-val')

    res_headers = session['response_headers']
    expect(res_headers['x-mock-header'] || res_headers['X-Mock-Header']).to eq('fiddler-test')
  end

  it 'captures POST request with payload and response status 200' do
    payload = JSON.generate({ message: 'ping', test: true })
    res = send_proxied_request(:post, '/echo_json', body: payload, headers: { 'Content-Type' => 'application/json' })
    expect(res.code).to eq('200')

    sessions = wait_for_session(expected_count: 1)
    session = sessions.last

    expect(session['method']).to eq('POST')
    expect(session['url']).to eq("http://127.0.0.1:#{mock_server.port}/echo_json")
    expect(session['status']).to eq(200)
    expect(session['request_body']).to eq(payload)
    expect(session['response_body']).to include('ping')
  end

  it 'captures 404 response status correctly' do
    res = send_proxied_request(:get, '/status/404')
    expect(res.code).to eq('404')

    sessions = wait_for_session(expected_count: 1)
    session = sessions.last

    expect(session['method']).to eq('GET')
    expect(session['url']).to eq("http://127.0.0.1:#{mock_server.port}/status/404")
    expect(session['status']).to eq(404)
    expect(session['response_body']).to eq('Not Found')
  end
end
