require 'socket'
require 'timeout'
require 'fileutils'
require 'json'

class FiddlerProcessManager
  DEFAULT_PORT = 8897
  DEFAULT_HOST = '127.0.0.1'
  DEFAULT_TIMEOUT = 10 # seconds

  attr_reader :host, :port, :project_root, :bin_path, :pid, :log_path

  def initialize(
    project_root: File.expand_path('..', __dir__),
    bin_path: nil,
    host: DEFAULT_HOST,
    port: DEFAULT_PORT,
    timeout: DEFAULT_TIMEOUT
  )
    @project_root = project_root
    @host = host
    @port = port
    @timeout = timeout
    @bin_path = bin_path || File.join(@project_root, 'target', 'debug', 'fiddler_clone_rs.exe')
    @log_path = File.join(@project_root, 'spec', 'fiddler_proxy.log')
    @pid = nil
  end

  def start
    return @pid if running?

    build_binary_if_missing!

    log_io = File.open(@log_path, 'a')
    @pid = Process.spawn(
      @bin_path,
      chdir: @project_root,
      out: log_io,
      err: log_io
    )
    log_io.close

    wait_until_ready(@timeout)
    @pid
  end

  def stop
    return unless @pid

    target_pid = @pid
    @pid = nil

    begin
      if Gem.win_platform?
        system("taskkill /F /T /PID #{target_pid} >nul 2>&1")
      else
        Process.kill('TERM', target_pid)
      end
    rescue StandardError => e
      warn "Failed to terminate process #{target_pid}: #{e.message}"
    end

    begin
      Timeout.timeout(5) do
        Process.wait(target_pid) rescue nil
      end
    rescue Timeout::Error
      if Gem.win_platform?
        system("taskkill /F /T /PID #{target_pid} >nul 2>&1")
      else
        Process.kill('KILL', target_pid) rescue nil
      end
      Process.wait(target_pid) rescue nil
    end

    wait_until_stopped(5)
    true
  end

  def restart
    stop
    start
  end

  def ready?(timeout = 1)
    Socket.tcp(@host, @port, connect_timeout: timeout) { true }
  rescue Errno::ECONNREFUSED, Errno::ETIMEDOUT, SocketError, Timeout::Error
    false
  end

  def running?
    return false unless @pid

    if Gem.win_platform?
      output = `tasklist /FI "PID eq #{@pid}" 2>nul`
      output.include?(@pid.to_s)
    else
      Process.kill(0, @pid)
      true
    end
  rescue Errno::ESRCH, Errno::EPERM
    false
  end

  def wait_until_ready(timeout = @timeout)
    start_time = Time.now
    loop do
      return true if ready?(0.5)

      if Time.now - start_time > timeout
        raise Timeout::Error, "proxy binary failed to become ready on #{@host}:#{@port} within #{timeout}s"
      end

      sleep 0.1
    end
  end

  def wait_until_stopped(timeout = 5)
    start_time = Time.now
    loop do
      return true unless ready?(0.2)

      if Time.now - start_time > timeout
        return false
      end

      sleep 0.1
    end
  end

  def sessions_file_path
    File.join(@project_root, 'sessions.json')
  end

  def clear_sessions!
    path = sessions_file_path
    File.write(path, '[]') if File.exist?(path)
  end

  def read_sessions
    path = sessions_file_path
    return [] unless File.exist?(path)

    content = File.read(path)
    return [] if content.strip.empty?

    JSON.parse(content)
  rescue JSON::ParserError
    []
  end

  private

  def build_binary_if_missing!
    return if File.exist?(@bin_path)

    system('cargo', 'build', '--bin', 'fiddler_clone_rs', chdir: @project_root)
    raise "Binary not found at #{@bin_path} and build failed" unless File.exist?(@bin_path)
  end
end
