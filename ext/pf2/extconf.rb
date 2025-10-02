require 'mkmf'
require 'mini_portile2'
require 'open3'
require 'fileutils'
require 'optparse'

gem_root = File.expand_path(File.join(File.dirname(__FILE__), '..', '..'))

options = {
  debug: ENV['PF2_DEBUG'] == '1',
}
option_parser = OptionParser.new do |opts|
  opts.on('--debug[=BOOL]') do |debug|
    options[:debug] =
      case debug
      when nil, "true"
        true
      when "false"
        false
      else
        raise OptionParser::InvalidArgument, "Expected true or false for --debug"
      end
  end
end
option_parser.parse!(ARGV)

libbacktrace = MiniPortile.new('libbacktrace', '1.0.0')
libbacktrace.source_directory = File.join(gem_root, 'vendor', 'libbacktrace')
libbacktrace.patch_files = Dir.glob(File.join(gem_root, 'ext', 'patches', 'libbacktrace', '*.patch'))
libbacktrace.configure_options << 'CFLAGS=-fPIC'

# Expand 'libbacktrace.cook' to call #patch on source_directory files
libbacktrace.prepare_build_directory
# Added: Copy source to build_directory
build_directory = libbacktrace.send(:work_path)
FileUtils.cp_r(File.join(libbacktrace.source_directory, '.'), build_directory)
libbacktrace.patch
libbacktrace.configure unless libbacktrace.configured?
libbacktrace.compile
libbacktrace.install unless libbacktrace.installed?
# END expand 'libbacktrace.cook'

libbacktrace.mkmf_config

if !have_func('backtrace_full', 'backtrace.h')
  raise 'libbacktrace has not been properly configured'
end

append_ldflags('-lrt') # for timer_create
append_cflags('-fvisibility=hidden')
append_cflags('-DPF2_DEBUG') if options[:debug]
append_cflags('-ggdb3')

# Check for timer functions
have_timer_create = have_func('timer_create')
have_setitimer = have_func('setitimer')

unless have_timer_create || have_setitimer
  raise 'Neither timer_create nor setitimer is available'
end

def gdb_exec(command)
  IO.pipe do |r, w|
    Process.detach(pid = fork { w.close; r.read })
    out, = Open3.capture2(*%W[gdb -pid #{pid} -batch -nx -ex #{command}])
    out
  end
end

def gdb_eval(expr)
  checking_for(expr) do
    out = gdb_exec("p #{expr}")
    if /^\$1 = (?:\((?<type>.+?)\) )?(?<value>.+)$/ =~ out
      value
    else
      raise out
    end
  end
end

$defs << "-DOFFSET_rb_callable_method_entry_t_def=#{gdb_eval('&((rb_callable_method_entry_t*)0)->def')}"
$defs << "-DOFFSET_rb_method_definition_t_type=#{gdb_eval('&((rb_method_definition_t*)0)->type')}"
$defs << "-DOFFSET_rb_method_definition_t_body_cfunc_func=#{gdb_eval('&((rb_method_definition_t*)0)->body.cfunc.func')}"
$defs << "-DVM_METHOD_TYPE_CFUNC=#{gdb_eval('(int)VM_METHOD_TYPE_CFUNC')}"

$srcs = Dir.glob("#{File.join(File.dirname(__FILE__), '*.c')}")
create_header
create_makefile 'pf2/pf2'
