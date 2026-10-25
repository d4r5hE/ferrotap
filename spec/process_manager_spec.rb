require_relative 'spec_helper'

RSpec.describe FiddlerProcessManager do
  subject(:manager) { described_class.new }

  after do
    manager.stop
  end

  it 'verifies that the target binary exists' do
    expect(File.exist?(manager.bin_path)).to be true
  end

  it 'starts the proxy process, reports readiness on port 8897, and tears down cleanly' do
    expect(manager.ready?).to be false

    pid = manager.start
    expect(pid).to be_a(Integer)
    expect(manager.running?).to be true
    expect(manager.ready?).to be true

    manager.stop
    expect(manager.ready?).to be false
    expect(manager.running?).to be false
  end
end
