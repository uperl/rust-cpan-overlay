use strict;
use warnings;
use Test::More tests => 1;
use Personal::Tools;
is(Personal::Tools::hello(), 'hello from the overlay');
