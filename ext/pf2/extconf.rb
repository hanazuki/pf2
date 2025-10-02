require 'mkmf'
require 'mini_portile2'
require 'open3'

libbacktrace = MiniPortile.new('libbacktrace', '1.0.0')
libbacktrace.source_directory = File.expand_path(File.join(File.dirname(__FILE__), '..', '..', 'vendor', 'libbacktrace'))
libbacktrace.configure_options << 'CFLAGS=-fPIC'
libbacktrace.cook
libbacktrace.mkmf_config

if !have_func('backtrace_full', 'backtrace.h')
  raise 'libbacktrace has not been properly configured'
end

append_ldflags('-lrt') # for timer_create
append_cflags('-fvisibility=hidden')
append_cflags('-DPF2_DEBUG') if ENV['PF2_DEBUG'] == '1'
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
