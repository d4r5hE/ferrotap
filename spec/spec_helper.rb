require 'rspec'
require_relative 'process_manager'

RSpec.configure do |config|
  config.expect_with :rspec do |expectations|
    expectations.include_chain_clauses_in_custom_matcher_descriptions = true
  end

  config.mock_with :rspec do |mocks|
    mocks.verify_partial_doubles = true
  end

  config.shared_context_metadata_behavior = :apply_to_host_groups
  config.filter_run_when_matching :focus
  config.example_status_persistence_file_path = File.expand_path('examples.txt', __dir__)
  config.disable_monkey_patching!
  config.order = :defined

  # Manage proxy process lifecycle if requested
  config.around(:each, :with_fiddler) do |example|
    manager = FiddlerProcessManager.new
    manager.clear_sessions!
    manager.start
    begin
      example.run
    ensure
      manager.stop
    end
  end
end

module FiddlerTestHelpers
  def fiddler
    @fiddler ||= FiddlerProcessManager.new
  end
end

RSpec.configure do |c|
  c.include FiddlerTestHelpers
end
