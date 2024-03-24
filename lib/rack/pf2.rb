require 'pf2'
require 'rack'

module Rack
  class Pf2
    DEFAULT_OPTIONS = {
      profiler_options: {},
      callback: nil,
    }

    def initialize(app, options = {})
      @app = app
      @options = DEFAULT_OPTIONS.merge(options)
    end

    def call(env)
      ::Pf2.start(**@options[:profiler_options])
      puts "rack: #{Process.pid}"
      response = @app.call(env)
      profile = ::Pf2.stop
      if @options[:callback]
        @options[:callback].call(profile)
      end
      response
    end
  end
end
