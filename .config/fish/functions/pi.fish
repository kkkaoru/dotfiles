function pi --description 'Run pi after recovering an unavailable working directory'
    __recover_working_directory
    or return 1
    command pi $argv
end
